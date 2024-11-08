use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::{mem, ptr, thread};
use std::thread::ThreadId;

use fxhash::FxBuildHasher;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;
use crate::WaitDuration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RwLockPtr(usize);

impl RwLockPtr {
    pub unsafe fn to_mut_ptr(self) -> *mut libc::pthread_rwlock_t {
        self.0 as *mut libc::pthread_rwlock_t
    }
}

impl From<*mut libc::pthread_rwlock_t> for RwLockPtr {
    fn from(value: *mut libc::pthread_rwlock_t) -> Self {
        RwLockPtr(value as usize)
    }
}

#[derive(Debug)]
pub struct RwLockInfo {
    pub kind: RwLockKind,
    pub state: RwLockState,
    pub awaiting_read: VecDeque<ThreadId>,
    pub awaiting_write: VecDeque<ThreadId>,
    /// The set of threads currently holding the RwLock.
    /// 
    /// POSIX specifies that read-write locks must allow for recursive reads (i.e., multiple held
    /// read locks by one thread). This is implemented using a HashMap with a counter for each
    /// thread holding the lock.
    /// 
    /// Only one thread should be in this when the state is `RwLockState::Writing`.
    pub holding_state: HashMap<ThreadId, usize, FxBuildHasher>,
}

impl RwLockInfo {
    fn new(kind: RwLockKind) -> Self {
        Self {
            kind,
            state: RwLockState::Available,
            awaiting_read: VecDeque::new(),
            awaiting_write: VecDeque::new(),
            holding_state: Default::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RwLockState {
    Available,
    Reading,
    Writing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RwLockKind {
    PreferReader,
    PreferWriter,
}

impl Display for RwLockKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RwLockKind::PreferReader => f.write_str("PTHREAD_RWLOCK_PREFER_READER_NP"),
            RwLockKind::PreferWriter => f.write_str("PTHREAD_RWLOCK_PREFER_WRITER_NP"),
        }
    }
}

pub struct RwLockInitEvent {
    rwlock: RwLockPtr,
    kind: RwLockKind,
}

impl RwLockInitEvent {
    pub fn new(rwlock: RwLockPtr, kind: RwLockKind) -> Self {
        Self {
            rwlock,
            kind,
        }
    }
}

impl Event for RwLockInitEvent {
    type Success = ();
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        if state.local.rwlocks.insert(self.rwlock, RwLockInfo::new(self.kind)).is_some() {
            panic!("[UB] `pthread_rwlock_init()` called twice on one rwlock");
        }

        Outcome::Success(())
    }
}

pub struct RwLockDestroyEvent {
    rwlock: RwLockPtr,
}

impl RwLockDestroyEvent {
    pub fn new(rwlock: RwLockPtr) -> Self {
        Self {
            rwlock,
        }
    }
}

impl Event for RwLockDestroyEvent {
    type Success = ();
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        match state.local.rwlocks.remove(&self.rwlock) {
            Some(rwlock_info) => {
                if rwlock_info.state != RwLockState::Available {
                    panic!("[UB] `pthread_rwlock_destroy` called on locked rwlock")
                }

                if !rwlock_info.awaiting_read.is_empty() || !rwlock_info.awaiting_write.is_empty() || !rwlock_info.holding_state.is_empty() {
                    panic!("inconsistent fizzle RwLock state in `pthread_rwlock_destroy`");
                }
            },
            None => {
                panic!("[UB] `pthread_rwlock_destroy` called on uninitialized rwlock")
                /*
                static RWLOCK_INIT: libc::pthread_rwlock_t = libc::PTHREAD_RWLOCK_INITIALIZER;

                // We need to find out if this lock is statically-initialized
                unsafe {
                    if libc::memcmp(self.rwlock.to_mut_ptr() as *const libc::c_void, ptr::addr_of!(RWLOCK_INIT) as *const libc::c_void, mem::size_of::<libc::pthread_rwlock_t>()) != 0 {
                        
                    }
                }

                let Some(kind) = static_mutex_kind(self.lock) else {
                    return Outcome::Error(Errno::EINVAL) // TODO: is this einval correct?
                };

                // This was a statically-initialized mutex--add it to our queue (and leave locked)
                let mut mutex_info = MutexInfo::new(kind, MutexRobustness::Stalled);
                mutex_info.queued_threads.push_back(thread::current().id());

                v.insert(mutex_info);
                return Outcome::Continue // Go to Finish state
                
                let res = libc::pthread_rwlock_trywrlock(lock);
                if res < 0 {
                    panic!("[UB] `pthread_rwlock_destroy` called on uninitialized rwlock")
                }
                */
            }
        };

        Outcome::Success(())
    }
}

pub enum RwLockReadState {
    Start,
    Finish,
}

pub struct RwLockReadEvent {
    rwlock: RwLockPtr,
    duration: WaitDuration,
    state: RwLockReadState,
}

impl RwLockReadEvent {
    pub fn new(rwlock: RwLockPtr, duration: WaitDuration) -> Self {
        Self {
            rwlock,
            duration,
            state: RwLockReadState::Start,
        }
    }
}

impl Event for RwLockReadEvent {
    type Success = ();
    type Error = Errno;

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {
        let current_thread = thread::current().id();

        match self.state {
            RwLockReadState::Start => {
                self.state = RwLockReadState::Finish;

                let rwlock_info = match state.local.rwlocks.get_mut(&self.rwlock) {
                    Some(rwlock_info) => rwlock_info,
                    None => {
                        static RWLOCK_INIT: libc::pthread_rwlock_t = libc::PTHREAD_RWLOCK_INITIALIZER;

                        // We need to find out if this lock is statically-initialized
                        unsafe {
                            if libc::memcmp(self.rwlock.to_mut_ptr() as *const libc::c_void, ptr::addr_of!(RWLOCK_INIT) as *const libc::c_void, mem::size_of::<libc::pthread_rwlock_t>()) != 0 {
                                panic!("[UB] read lock called on uninitialized rwlock")
                            }
                        }

                        // This was a statically-initialized rwlock--add it to our internal state
                        state.local.rwlocks.insert(self.rwlock, RwLockInfo::new(RwLockKind::PreferReader));
                        state.local.rwlocks.get_mut(&self.rwlock).unwrap()
                    }
                };

                match rwlock_info.state {
                    RwLockState::Writing => match self.duration {
                        WaitDuration::Immediate => Outcome::Error(Errno::EBUSY),
                        WaitDuration::Timed(duration) => {
                            rwlock_info.awaiting_read.push_back(current_thread);
                            Outcome::Yield(Some(duration))
                        }
                        WaitDuration::Indefinite => {
                            rwlock_info.awaiting_read.push_back(current_thread);
                            Outcome::Yield(None)
                        }
                    }
                    // We have a pending writer, and this RwLock is configured to prioritize writes
                    RwLockState::Reading if rwlock_info.kind == RwLockKind::PreferWriter && !rwlock_info.awaiting_write.is_empty() => match self.duration {
                        WaitDuration::Immediate => Outcome::Error(Errno::EBUSY),
                        WaitDuration::Timed(duration) => {
                            rwlock_info.awaiting_read.push_back(current_thread);
                            Outcome::Yield(Some(duration))
                        }
                        WaitDuration::Indefinite => {
                            rwlock_info.awaiting_read.push_back(current_thread);
                            Outcome::Yield(None)
                        }
                    }
                    RwLockState::Reading => {
                        // The lock is ready to be taken
                        match rwlock_info.holding_state.entry(current_thread) {
                            // Recursive read--increment counter for this thread
                            Entry::Occupied(mut o) => *o.get_mut() += 1,
                            // Non-recursive read--insert thread into map
                            Entry::Vacant(v) => {
                                v.insert(1);
                            }
                        }
                        Outcome::Success(())
                    },
                    RwLockState::Available => {
                        if !rwlock_info.holding_state.is_empty() {
                            panic!("fizzle RwLock in inconsistent state (RwLockState::Available when some threads still holding state)");
                        }

                        rwlock_info.state = RwLockState::Reading;
                        rwlock_info.holding_state.insert(current_thread, 1);
                        Outcome::Success(())
                    }
                }
            }
            RwLockReadState::Finish => {
                let rwlock_info = state.local.rwlocks.get_mut(&self.rwlock).unwrap();
                if rwlock_info.holding_state.contains_key(&current_thread) {
                    Outcome::Success(())

                } else {
                    // Remove the thread from the read lock queue
                    for (i, thread_id) in rwlock_info.awaiting_read.iter().enumerate() {
                        if *thread_id == current_thread {
                            rwlock_info.awaiting_read.remove(i).unwrap();
                            break
                        }
                    }

                    Outcome::Error(Errno::ETIMEDOUT)
                }
            }
        }
    }
}

pub enum RwLockWriteState {
    Start,
    Finish,
}

pub struct RwLockWriteEvent {
    rwlock: RwLockPtr,
    duration: WaitDuration,
    state: RwLockWriteState,
}

impl RwLockWriteEvent {
    pub fn new(rwlock: RwLockPtr, duration: WaitDuration) -> Self {
        Self {
            rwlock,
            duration,
            state: RwLockWriteState::Start,
        }
    }
}

impl Event for RwLockWriteEvent {
    type Success = ();
    type Error = Errno;

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {
        let current_thread = thread::current().id();

        match self.state {
            RwLockWriteState::Start => {
                self.state = RwLockWriteState::Finish;

                let rwlock_info = match state.local.rwlocks.get_mut(&self.rwlock) {
                    Some(rwlock_info) => rwlock_info,
                    None => {
                        static RWLOCK_INIT: libc::pthread_rwlock_t = libc::PTHREAD_RWLOCK_INITIALIZER;

                        // We need to find out if this lock is statically-initialized
                        unsafe {
                            if libc::memcmp(self.rwlock.to_mut_ptr() as *const libc::c_void, ptr::addr_of!(RWLOCK_INIT) as *const libc::c_void, mem::size_of::<libc::pthread_rwlock_t>()) != 0 {
                                panic!("[UB] read lock called on uninitialized rwlock")
                            }
                        }

                        // This was a statically-initialized rwlock--add it to our internal state
                        state.local.rwlocks.insert(self.rwlock, RwLockInfo::new(RwLockKind::PreferReader));
                        state.local.rwlocks.get_mut(&self.rwlock).unwrap()
                    }
                };

                match rwlock_info.state {
                    RwLockState::Reading | RwLockState::Writing => match self.duration {
                        WaitDuration::Immediate => Outcome::Error(Errno::EBUSY),
                        WaitDuration::Timed(duration) => {
                            rwlock_info.awaiting_write.push_back(current_thread);
                            Outcome::Yield(Some(duration))
                        }
                        WaitDuration::Indefinite => {
                            rwlock_info.awaiting_write.push_back(current_thread);
                            Outcome::Yield(None)
                        }
                    }
                    RwLockState::Available => {
                        if !rwlock_info.holding_state.is_empty() {
                            panic!("fizzle RwLock in inconsistent state (RwLockState::Available when some threads still holding state)");
                        }

                        rwlock_info.state = RwLockState::Writing;
                        rwlock_info.holding_state.insert(current_thread, 1);
                        Outcome::Success(())
                    }
                }
            }
            RwLockWriteState::Finish => {
                let rwlock_info = state.local.rwlocks.get_mut(&self.rwlock).unwrap();
                if rwlock_info.holding_state.contains_key(&current_thread) {
                    Outcome::Success(())

                } else {
                    // Remove the thread from the read lock queue
                    for (i, thread_id) in rwlock_info.awaiting_write.iter().enumerate() {
                        if *thread_id == current_thread {
                            rwlock_info.awaiting_write.remove(i).unwrap();
                            break
                        }
                    }

                    Outcome::Error(Errno::ETIMEDOUT)
                }
            }
        }
    }
}

pub struct RwLockUnlockEvent {
    rwlock: RwLockPtr,
}

impl RwLockUnlockEvent {
    pub fn new(rwlock: RwLockPtr) -> Self {
        Self {
            rwlock,
        }
    }
}

impl Event for RwLockUnlockEvent {
    type Success = ();
    type Error = Errno;

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {
        let current_thread = thread::current().id();

        let Some(rwlock_info) = state.local.rwlocks.get_mut(&self.rwlock) else {
            panic!("[UB] `pthread_rwlock_unlock` called on uninitialized rwlock")
        };

        match rwlock_info.holding_state.entry(current_thread) {
            Entry::Occupied(mut o) => {
                *o.get_mut() -= 1;
                if *o.get() == 0 {
                    // All held locks for this thread have been released
                    o.remove();
                }
            }
            Entry::Vacant(_) => panic!("[UB] `pthread_rwlock_unlock` called on rwlock when current_thread not holding lock"),
        }

        if !rwlock_info.holding_state.is_empty() {
            return Outcome::Success(())
        }

        // No more threads holding lock--time to transition to a new state
        match rwlock_info.state {
            RwLockState::Available => panic!("Inconsistent Fizzle state--rwlock unexpectedly available during pthread_rwlock_unlock()"),
            // All readers are done--see if any writers are waiting on the lock
            RwLockState::Reading => match rwlock_info.awaiting_write.pop_front() {
                Some(write_thread) => {
                    rwlock_info.holding_state.insert(write_thread, 1);
                    rwlock_info.state = RwLockState::Writing;
                    state.mark_thread_ready(write_thread);
                }
                None => {
                    // There should only be threads awaiting reads if the RwLock was configured to
                    // Prioritize writes *and* there was a thread awaiting a write
                    assert!(rwlock_info.awaiting_read.is_empty());
                    rwlock_info.state = RwLockState::Available;
                }
            }
            RwLockState::Writing if (rwlock_info.kind == RwLockKind::PreferReader || rwlock_info.awaiting_write.is_empty()) && !rwlock_info.awaiting_read.is_empty() => {
                rwlock_info.state = RwLockState::Reading;

                let mut awaiting_read = VecDeque::new();
                mem::swap(&mut awaiting_read, &mut rwlock_info.awaiting_read);

                for &thread_id in awaiting_read.iter() {
                    rwlock_info.holding_state.insert(thread_id, 1);
                }

                for thread_id in awaiting_read {
                    state.mark_thread_ready(thread_id);
                }
            }
            RwLockState::Writing => {
                if let Some(thread_id) = rwlock_info.awaiting_write.pop_front() {
                    state.mark_thread_ready(thread_id);

                } else {
                    // No threads awaiting reads or writes
                    rwlock_info.state = RwLockState::Available;
                }
            }
        }

        Outcome::Success(())
    }
}
