use std::collections::{hash_map::Entry, VecDeque};
use std::fmt::Display;
use std::thread::ThreadId;
use std::{mem, ptr, thread};

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::WaitDuration;

// TODO: need to support static mutex initialization

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutexPtr(usize);

impl MutexPtr {
    pub unsafe fn to_mut_ptr(self) -> *mut libc::pthread_mutex_t {
        self.0 as *mut libc::pthread_mutex_t
    }
}

impl From<*mut libc::pthread_mutex_t> for MutexPtr {
    fn from(value: *mut libc::pthread_mutex_t) -> Self {
        MutexPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutexKind {
    Fast,
    Recursive,
    ErrorChecking,
}

impl Display for MutexKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fast => f.write_str("PTHREAD_MUTEX_FAST_NP"),
            Self::ErrorChecking => f.write_str("PTHREAD_MUTEX_ERRORCHECK_NP"),
            Self::Recursive => f.write_str("PTHREAD_MUTEX_RECURSIVE_NP"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutexRobustness {
    Stalled,
    Robust,
}

impl Display for MutexRobustness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stalled => f.write_str("PTHREAD_MUTEX_STALLED"),
            Self::Robust => f.write_str("PTHREAD_MUTEX_ROBUST"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutexStatus {
    /// The mutex can be used normally.
    Ready,
    /// A thread has exited while holding the mutex.
    Poisoned,
    /// A thread has exited while holding the mutex, and the next thread to receive the mutex did
    /// not recover it.
    Unusable,
}

#[derive(Debug)]
pub struct MutexInfo {
    pub kind: MutexKind,
    pub robustness: MutexRobustness,
    pub queued_threads: VecDeque<ThreadId>,
    /// Indicates whether the mutex has been poisoned or rendered unusable by an exiting thread.
    pub status: MutexStatus,
}

impl MutexInfo {
    pub fn new(kind: MutexKind, robustness: MutexRobustness) -> Self {
        Self {
            kind,
            robustness,
            queued_threads: VecDeque::new(),
            status: MutexStatus::Ready,
        }
    }
}

pub struct MutexInitEvent {
    lock: MutexPtr,
    kind: MutexKind,
    robustness: MutexRobustness,
}

impl MutexInitEvent {
    pub fn new(lock: MutexPtr, kind: MutexKind, robustness: MutexRobustness) -> Self {
        Self {
            lock,
            kind,
            robustness,
        }
    }
}

impl Event for MutexInitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if state
            .local
            .mutexes
            .insert(self.lock, MutexInfo::new(self.kind, self.robustness))
            .is_some()
        {
            panic!("[UB] `pthread_mutex_init()` called twice on one mutex");
        }

        Outcome::Success(())
    }
}

pub struct MutexDestroyEvent {
    lock: MutexPtr,
}

impl MutexDestroyEvent {
    pub fn new(lock: MutexPtr) -> Self {
        Self { lock }
    }
}

impl Event for MutexDestroyEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(mutex_info) = state.local.mutexes.get(&self.lock) else {
            panic!("[UB] `pthread_mutex_destroy()` called on uninitialized mutex");
        };

        if !mutex_info.queued_threads.is_empty() {
            Outcome::Error(Errno::EBUSY)
        } else {
            let _ = state.local.mutexes.remove(&self.lock);
            Outcome::Success(())
        }
    }
}

pub enum MutexLockState {
    Start,
    Finish,
}

pub struct MutexLockEvent {
    state: MutexLockState,
    lock: MutexPtr,
    wait: WaitDuration,
}

impl MutexLockEvent {
    pub fn new(lock: MutexPtr, wait: WaitDuration) -> Self {
        Self {
            state: MutexLockState::Start,
            lock,
            wait,
        }
    }
}

impl Event for MutexLockEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_thread = thread::current().id();

        match self.state {
            MutexLockState::Start => {
                self.state = MutexLockState::Finish;

                match state.local.mutexes.entry(self.lock) {
                    Entry::Occupied(mut o) => {
                        let mutex_info = o.get_mut();
                        match mutex_info.status {
                            MutexStatus::Ready => (),
                            MutexStatus::Poisoned => {
                                // This state is set when the owner of the mutex dies.

                                // Make the current thread the owner of the mutex
                                mutex_info.queued_threads.push_front(current_thread);

                                return Outcome::Yield(YieldUntil::Immediate); // Go to Finish state to return poisoned lock
                            }
                            MutexStatus::Unusable => return Outcome::Yield(YieldUntil::Immediate), // Go to Finish state
                        }

                        if mutex_info.queued_threads.is_empty() {
                            // Mutex is immediately available
                            mutex_info.queued_threads.push_back(current_thread);

                            return Outcome::Yield(YieldUntil::Immediate);
                        }

                        let holding_thread = *mutex_info.queued_threads.front().unwrap();
                        if holding_thread == current_thread {
                            match mutex_info.kind {
                                // Suspend calling thread forever
                                MutexKind::Fast => {
                                    log::error!(
                                        "[Deadlock] Thread locking a mutex it already holds"
                                    );
                                    Outcome::Yield(YieldUntil::None)
                                }
                                // Return successfully immediately
                                MutexKind::Recursive => Outcome::Yield(YieldUntil::Immediate),
                                // Return a deadlock error
                                MutexKind::ErrorChecking => Outcome::Error(Errno::EDEADLK),
                            }
                        } else {
                            match self.wait {
                                WaitDuration::Immediate => Outcome::Error(Errno::EBUSY),
                                WaitDuration::Timed(duration) => {
                                    mutex_info.queued_threads.push_back(current_thread);
                                    Outcome::Yield(YieldUntil::Reschedule(duration))
                                }
                                WaitDuration::Indefinite => {
                                    mutex_info.queued_threads.push_back(current_thread);
                                    Outcome::Yield(YieldUntil::None)
                                }
                            }
                        }
                    }
                    Entry::Vacant(v) => {
                        let Some(kind) = static_mutex_kind(self.lock) else {
                            return Outcome::Error(Errno::EINVAL); // TODO: is this einval correct?
                        };

                        // This was a statically-initialized mutex--add it to our queue (and leave locked)
                        let mut mutex_info = MutexInfo::new(kind, MutexRobustness::Stalled);
                        mutex_info.queued_threads.push_back(current_thread);

                        v.insert(mutex_info);
                        return Outcome::Yield(YieldUntil::Immediate); // Go to Finish state
                    }
                }
            }
            MutexLockState::Finish => {
                let Some(mutex) = state.local.mutexes.get_mut(&self.lock) else {
                    panic!("internal Fizzle error: mutex destroyed while being waited on");
                };

                if mutex.queued_threads.front() != Some(&current_thread) {
                    // This thread isn't designated as the owner of the mutex...

                    if mutex.status == MutexStatus::Unusable {
                        // ...because the thread was poisoned and not recovered.
                        return Outcome::Error(Errno::ENOTRECOVERABLE);
                    }

                    // ...because there was a timeout.
                    for (idx, thread_id) in mutex.queued_threads.iter().enumerate() {
                        if *thread_id == current_thread {
                            mutex.queued_threads.remove(idx).unwrap();
                            break;
                        }
                    }

                    // The worker was still waiting--this must have been a timeout wakeup
                    let WaitDuration::Timed(t) = self.wait else {
                        panic!("internal Fizzle error: mutex awakened despite thread still being in queue");
                    };

                    log::debug!("mutex timed lock timed out after {:?}", t);
                    return Outcome::Error(Errno::ETIMEDOUT);
                }

                match mutex.status {
                    MutexStatus::Ready => {
                        // Mark the thread as being owned by the current process
                        state
                            .local
                            .pthreads
                            .get_mut(unsafe { &libc::pthread_self() })
                            .unwrap()
                            .held_mutexes
                            .insert(self.lock);

                        Outcome::Success(())
                    }
                    MutexStatus::Poisoned => {
                        // Mark the thread as being owned by the current process
                        state
                            .local
                            .pthreads
                            .get_mut(unsafe { &libc::pthread_self() })
                            .unwrap()
                            .held_mutexes
                            .insert(self.lock);

                        Outcome::Error(Errno::EOWNERDEAD)
                    }
                    MutexStatus::Unusable => Outcome::Error(Errno::ENOTRECOVERABLE),
                }
            }
        }
    }
}

pub fn static_mutex_kind(mutex: MutexPtr) -> Option<MutexKind> {
    static FAST_INIT: libc::pthread_mutex_t = libc::PTHREAD_MUTEX_INITIALIZER;
    static RECURSIVE_INIT: libc::pthread_mutex_t = libc::PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP;
    static ERRORCHECK_INIT: libc::pthread_mutex_t = libc::PTHREAD_ERRORCHECK_MUTEX_INITIALIZER_NP;

    // We need to find out if this lock is statically-initialized
    unsafe {
        if libc::memcmp(
            mutex.to_mut_ptr().cast::<libc::c_void>(),
            ptr::addr_of!(FAST_INIT).cast::<libc::c_void>(),
            mem::size_of::<libc::pthread_mutex_t>(),
        ) == 0
        {
            Some(MutexKind::Fast)
        } else if libc::memcmp(
            mutex.to_mut_ptr().cast::<libc::c_void>(),
            ptr::addr_of!(RECURSIVE_INIT).cast::<libc::c_void>(),
            mem::size_of::<libc::pthread_mutex_t>(),
        ) == 0
        {
            Some(MutexKind::Recursive)
        } else if libc::memcmp(
            mutex.to_mut_ptr().cast::<libc::c_void>(),
            ptr::addr_of!(ERRORCHECK_INIT).cast::<libc::c_void>(),
            mem::size_of::<libc::pthread_mutex_t>(),
        ) == 0
        {
            Some(MutexKind::ErrorChecking)
        } else {
            None
        }
    }
}

pub struct MutexUnlockEvent {
    lock: MutexPtr,
}

impl MutexUnlockEvent {
    pub fn new(lock: MutexPtr) -> Self {
        Self { lock }
    }
}

impl Event for MutexUnlockEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(mutex_info) = state.local.mutexes.get_mut(&self.lock) else {
            return Outcome::Error(Errno::EINVAL);
        };

        let Some(popped_thread) = mutex_info.queued_threads.front().cloned() else {
            return Outcome::Error(Errno::EINVAL);
        };

        if popped_thread != thread::current().id() {
            if mutex_info.kind == MutexKind::ErrorChecking {
                return Outcome::Error(Errno::EINVAL);
            } else {
                panic!("[UB] `pthread_mutex_unlock()` called by a thread not currently holding the mutex")
            }
        }

        mutex_info.queued_threads.pop_front();

        // Mark the thread as no longer being owned by the current process
        state
            .local
            .pthreads
            .get_mut(unsafe { &libc::pthread_self() })
            .unwrap()
            .held_mutexes
            .remove(&self.lock);

        match mutex_info.status {
            MutexStatus::Ready => (),
            MutexStatus::Poisoned => {
                // A poisoned mutex has been unlocked before being recovered--now unusable
                mutex_info.status = MutexStatus::Unusable;

                // Let all other threads waiting on this mutex know that it's unusable
                let mut queued_threads = VecDeque::new();
                mem::swap(&mut queued_threads, &mut mutex_info.queued_threads);
                while let Some(thread_id) = queued_threads.pop_front() {
                    state.mark_thread_ready(thread_id);
                }

                return Outcome::Success(());
            }
            MutexStatus::Unusable => unreachable!(),
        }

        if let Some(next_thread) = mutex_info.queued_threads.front().copied() {
            state.mark_thread_ready(next_thread);
        }

        Outcome::Success(())
    }
}

pub struct MutexConsistentEvent {
    lock: MutexPtr,
}

impl MutexConsistentEvent {
    pub fn new(lock: MutexPtr) -> Self {
        Self { lock }
    }
}

impl Event for MutexConsistentEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(mutex_info) = state.local.mutexes.get_mut(&self.lock) else {
            panic!("[UB] `pthread_mutex_consistent()` called on uninitialized mutex")
        };

        if mutex_info.robustness != MutexRobustness::Robust {
            return Outcome::Error(Errno::EINVAL);
        }

        // TODO: is this important to enforce? Man page isn't clear...
        /*
        if popped_thread != thread::current().id() {
            if mutex_info.kind == MutexKind::ErrorChecking {
                return Outcome::Error(Errno::EINVAL)
            } else {
                panic!("[UB] `pthread_mutex_unlock()` called by a thread not currently holding the mutex")
            }
        }
        */

        match mutex_info.status {
            MutexStatus::Ready => Outcome::Error(Errno::EINVAL),
            MutexStatus::Poisoned => {
                mutex_info.status = MutexStatus::Ready;
                Outcome::Success(())
            }
            MutexStatus::Unusable => Outcome::Error(Errno::EINVAL),
        }
    }
}
