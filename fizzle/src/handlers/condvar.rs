use std::collections::{HashSet, VecDeque};
use std::thread::ThreadId;
use std::{mem, ptr, thread};

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::WaitDuration;

use super::mutex::MutexPtr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CondVarPtr(usize);

impl CondVarPtr {
    pub unsafe fn to_mut_ptr(self) -> *mut libc::pthread_cond_t {
        self.0 as *mut libc::pthread_cond_t
    }
}

impl From<*mut libc::pthread_cond_t> for CondVarPtr {
    fn from(value: *mut libc::pthread_cond_t) -> Self {
        CondVarPtr(value as usize)
    }
}

#[derive(Debug, Clone)]
pub struct CondVarInfo {
    waiting: VecDeque<ThreadId>,
    ready: HashSet<ThreadId>,
}

impl CondVarInfo {
    pub fn new() -> Self {
        Self {
            waiting: Default::default(),
            ready: Default::default(),
        }
    }
}

pub struct CondInitEvent {
    cond: CondVarPtr,
}

impl CondInitEvent {
    pub fn new(lock: CondVarPtr) -> Self {
        Self { cond: lock }
    }
}

impl Event for CondInitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if state
            .local
            .condvars
            .insert(self.cond, CondVarInfo::new())
            .is_some()
        {
            panic!("[UB] `pthread_cond_init()` called twice on one condvar");
        }

        Outcome::Success(())
    }
}

pub struct CondDestroyEvent {
    cond: CondVarPtr,
}

impl CondDestroyEvent {
    pub fn new(lock: CondVarPtr) -> Self {
        Self { cond: lock }
    }
}

impl Event for CondDestroyEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(cond_info) = state.local.condvars.remove(&self.cond) else {
            panic!("[UB] `pthread_cond_destroy()` called on uninitialized condvar");
        };

        if !cond_info.waiting.is_empty() {
            panic!("[UB] `pthread_cond_destroy` called on condvar being waited on");
        }

        if !cond_info.ready.is_empty() {
            panic!("[UB] `pthread_cond_destroy` called on condvar with unawakened threads");
        }

        Outcome::Success(())
    }
}

pub struct CondSignalEvent {
    cond: CondVarPtr,
}

impl CondSignalEvent {
    pub fn new(lock: CondVarPtr) -> Self {
        Self { cond: lock }
    }
}

impl Event for CondSignalEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        static COND_STATIC_INIT: libc::pthread_cond_t = libc::PTHREAD_COND_INITIALIZER;

        if let Some(cond_info) = state.local.condvars.get_mut(&self.cond) {
            if let Some(thread_id) = cond_info.waiting.pop_front() {
                cond_info.ready.insert(thread_id);
                state.mark_thread_ready(thread_id);
            }
        } else {
            if unsafe {
                libc::memcmp(
                    self.cond.to_mut_ptr().cast::<libc::c_void>(),
                    ptr::addr_of!(COND_STATIC_INIT).cast::<libc::c_void>(),
                    mem::size_of::<libc::pthread_cond_t>(),
                ) == 0
            } {
                state.local.condvars.insert(self.cond, CondVarInfo::new());
                // If no threads are waiting, nothing else happens
            } else {
                panic!("[UB] `pthread_cond_signal` called on uninitialized condvar")
            }
        };

        Outcome::Success(())
    }
}

pub struct CondBroadcastEvent {
    cond: CondVarPtr,
}

impl CondBroadcastEvent {
    pub fn new(lock: CondVarPtr) -> Self {
        Self { cond: lock }
    }
}

impl Event for CondBroadcastEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        static COND_STATIC_INIT: libc::pthread_cond_t = libc::PTHREAD_COND_INITIALIZER;

        if let Some(cond_info) = state.local.condvars.get_mut(&self.cond) {
            let mut waiting_queue = VecDeque::new();
            mem::swap(&mut waiting_queue, &mut cond_info.waiting);

            for thread in waiting_queue.iter() {
                cond_info.ready.insert(*thread);
            }

            for thread in waiting_queue {
                state.mark_thread_ready(thread);
            }
        } else {
            if unsafe {
                libc::memcmp(
                    self.cond.to_mut_ptr().cast::<libc::c_void>(),
                    ptr::addr_of!(COND_STATIC_INIT).cast::<libc::c_void>(),
                    mem::size_of::<libc::pthread_cond_t>(),
                ) == 0
            } {
                state.local.condvars.insert(self.cond, CondVarInfo::new());
                // If no threads are waiting, nothing else happens
            } else {
                panic!("[UB] `pthread_cond_signal` called on uninitialized condvar")
            }
        };

        Outcome::Success(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CondWaitState {
    Start,
    AwaitCond,
    Finish,
}

pub struct CondWaitEvent {
    cond: CondVarPtr,
    mutex: MutexPtr,
    duration: WaitDuration,
    state: CondWaitState,
}

impl CondWaitEvent {
    pub fn new(cond: CondVarPtr, mutex: MutexPtr, duration: WaitDuration) -> Self {
        Self {
            cond,
            mutex,
            duration,
            state: CondWaitState::Start,
        }
    }
}

impl Event for CondWaitEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        static COND_STATIC_INIT: libc::pthread_cond_t = libc::PTHREAD_COND_INITIALIZER;

        match self.state {
            CondWaitState::Start => {
                self.state = CondWaitState::AwaitCond;

                let cond_info = match state.local.condvars.get_mut(&self.cond) {
                    Some(info) => info,
                    None => {
                        if unsafe {
                            libc::memcmp(
                                self.cond.to_mut_ptr().cast::<libc::c_void>(),
                                ptr::addr_of!(COND_STATIC_INIT).cast::<libc::c_void>(),
                                mem::size_of::<libc::pthread_cond_t>(),
                            ) == 0
                        } {
                            // This was a statically-initialized mutex--add it to our queue (and leave locked)
                            state.local.condvars.insert(self.cond, CondVarInfo::new());
                            state.local.condvars.get_mut(&self.cond).unwrap()
                        } else {
                            panic!("[UB] `pthread_cond_signal` called on uninitialized condvar")
                        }
                    }
                };

                cond_info.waiting.push_back(thread::current().id());

                // Now unlock the mutex
                let mutex_info = match state.local.mutexes.get_mut(&self.mutex) {
                    Some(mutex_info) => mutex_info,
                    None => {
                        panic!("[UB] `pthread_cond_wait()` called on uninitialized/empty mutex");

                        /*
                        // This was a statically-initialized mutex--add it to our queue (and leave locked)
                        let mut mutex_info = MutexInfo::new(kind, MutexRobustness::Stalled);
                        state.local.mutexes.insert(self.mutex, mutex_info);
                        state.local.mutexes.get_mut(&self.mutex).unwrap()
                        */
                    }
                };

                let Some(popped_thread) = mutex_info.queued_threads.pop_front() else {
                    panic!("[UB] `pthread_cond_wait` called when mutex already unlocked")
                };

                if popped_thread != thread::current().id() {
                    panic!("[UB] `pthread_cond_wait` called by a thread not currently holding the mutex lock")
                }

                if let Some(next_thread) = mutex_info.queued_threads.front().copied() {
                    state.mark_thread_ready(next_thread);
                }

                match self.duration {
                    WaitDuration::Immediate => unreachable!(),
                    WaitDuration::Timed(timeout) => Outcome::Yield(YieldUntil::Reschedule(timeout)),
                    WaitDuration::Indefinite => Outcome::Yield(YieldUntil::None),
                }
            }
            CondWaitState::AwaitCond => {
                self.state = CondWaitState::Finish;

                // Issue: this is broken when pthread_cond_broadcast is used, or when signal is used multiple times
                if state
                    .local
                    .condvars
                    .get_mut(&self.cond)
                    .map(|c| c.ready.remove(&thread::current().id()))
                    != Some(true)
                {
                    return Outcome::Error(Errno::ETIMEDOUT);
                }

                let Some(mutex_info) = state.local.mutexes.get_mut(&self.mutex) else {
                    panic!("[UB] `pthread_cond_clockwait` mutex freed while waiting for condition")
                };

                let available = mutex_info.queued_threads.is_empty();
                mutex_info.queued_threads.push_back(thread::current().id());

                if available {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    match self.duration {
                        WaitDuration::Immediate => unreachable!(),
                        WaitDuration::Timed(timeout) => {
                            Outcome::Yield(YieldUntil::Reschedule(timeout))
                        }
                        WaitDuration::Indefinite => Outcome::Yield(YieldUntil::None),
                    }
                }
            }
            CondWaitState::Finish => Outcome::Success(()),
        }
    }
}
