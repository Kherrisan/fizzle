use std::{collections::VecDeque, thread};

use crate::{
    errno::Errno,
    scheduler::{Event, Outcome, YieldUntil},
    state::FizzleState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpinlockPtr(usize);

impl From<*mut libc::pthread_spinlock_t> for SpinlockPtr {
    fn from(value: *mut libc::pthread_spinlock_t) -> Self {
        SpinlockPtr(value as usize)
    }
}

pub struct ThreadSpinInitEvent {
    lock: SpinlockPtr,
    shared: bool,
}

impl ThreadSpinInitEvent {
    pub fn new(lock: SpinlockPtr, shared: bool) -> Self {
        Self { lock, shared }
    }
}

impl Event for ThreadSpinInitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if self.shared {
            log::warn!("Process-shared spinlocks not implemented");
        }

        if state
            .local
            .spinlocks
            .insert(self.lock, VecDeque::new())
            .is_some()
        {
            panic!("[UB] `pthread_spin_init()` called twice on one spinlock");
        }

        Outcome::Success(())
    }
}

pub struct ThreadSpinDestroyEvent {
    lock: SpinlockPtr,
}

impl ThreadSpinDestroyEvent {
    pub fn new(lock: SpinlockPtr) -> Self {
        Self { lock }
    }
}

impl Event for ThreadSpinDestroyEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(spinlock_queue) = state.local.spinlocks.remove(&self.lock) else {
            panic!("[UB] `pthread_spin_destroy()` called on uninitialized spinlock");
        };

        if !spinlock_queue.is_empty() {
            panic!("[UB] `pthread_spin_destroy()` called on locked spinlock");
        }

        Outcome::Success(())
    }
}

pub enum ThreadSpinLockState {
    Start,
    Finish,
}

pub struct ThreadSpinLockEvent {
    state: ThreadSpinLockState,
    lock: SpinlockPtr,
    nonblocking: bool,
}

impl ThreadSpinLockEvent {
    pub fn new(lock: SpinlockPtr, nonblocking: bool) -> Self {
        Self {
            state: ThreadSpinLockState::Start,
            lock,
            nonblocking,
        }
    }
}

impl Event for ThreadSpinLockEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            ThreadSpinLockState::Start => {
                self.state = ThreadSpinLockState::Finish;

                let Some(spinlock_queue) = state.local.spinlocks.get_mut(&self.lock) else {
                    panic!("[UB] `pthread_spin_lock()` called on uninitialized spinlock")
                };

                if spinlock_queue.is_empty() {
                    // Spinlock is immediately available
                    spinlock_queue.push_back(thread::current().id());
                    Outcome::Success(())
                } else if self.nonblocking {
                    Outcome::Error(Errno::EBUSY)
                } else {
                    spinlock_queue.push_back(thread::current().id());
                    Outcome::Yield(YieldUntil::None)
                }
            }
            ThreadSpinLockState::Finish => Outcome::Success(()),
        }
    }
}

pub struct ThreadSpinUnlockEvent {
    lock: SpinlockPtr,
}

impl ThreadSpinUnlockEvent {
    pub fn new(lock: SpinlockPtr) -> Self {
        Self { lock }
    }
}

impl Event for ThreadSpinUnlockEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(spinlock_queue) = state.local.spinlocks.get_mut(&self.lock) else {
            panic!("[UB] `pthread_spin_unlock()` called on uninitialized spinlock")
        };

        let Some(popped_thread) = spinlock_queue.pop_front() else {
            panic!("[UB] `pthread_spin_unlock()` called when spinlock already unlocked")
        };

        if popped_thread != thread::current().id() {
            panic!("[UB] `pthread_spin_unlock()` called by a thread not currently holding the spinlock")
        }

        if let Some(next_thread) = spinlock_queue.front().copied() {
            state.mark_thread_ready(next_thread);
        }

        Outcome::Success(())
    }
}
