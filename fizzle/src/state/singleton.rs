use std::{marker::PhantomData, thread::ThreadId};

use fizzle_common::storage::Rc;

use crate::semaphore::Semaphore;

use super::{FizzCell, FizzGuard, PolledId, ThreadTermination};

static FIZZLE_STATE: FizzCell = FizzCell::new();

pub struct FizzleSingleton {
    _phantom: PhantomData<()>,
}

impl FizzleSingleton {
    /// Acquires the global shared state for mutable access.
    pub fn acquire(&mut self) -> FizzGuard<'_> {
        FIZZLE_STATE.acquire()
    }

    pub fn init_thread_lock(&self, thread_id: &ThreadId) {
        FIZZLE_STATE.init_thread_lock(thread_id)
    }

    pub fn get_thread_lock(&mut self, thread_id: &ThreadId) -> &Semaphore {
        FIZZLE_STATE.get_thread_lock(thread_id)
    }

    pub fn init_new_thread(&mut self) {
        FIZZLE_STATE.init_new_thread()
    }

    pub fn yield_thread(&mut self) {
        FIZZLE_STATE.yield_thread()
    }

    pub fn terminate_thread(&mut self, term_method: ThreadTermination) -> ! {
        FIZZLE_STATE.terminate_thread(term_method)
    }

    pub fn pause_current_thread(&mut self) {
        FIZZLE_STATE.pause_current_thread()
    }

    pub fn pause_current_process(&mut self) {
        FIZZLE_STATE.pause_current_process()
    }

    pub fn poll_until_ready(&mut self, polled_id: Rc<PolledId>) {
        FIZZLE_STATE.poll_until_ready(polled_id)
    }
}

// Pass the fizzle singleton around into various subsystems




/// Produces a new `FizzleSingleton` instance that can be used to acquire global state in a safe manner.
/// 
/// WARNING: this function SHOULD NOT be used in any methods other than a) the hook macro, or b)
/// those that create new threads, such as `pthread_create`. The `FizzleSingleton` is designed to
/// ensure that the global `FIZZLE_STATE` variable is never mutably referenced more than once. A
/// single instantiation of it is provided for each LD_PRELOAD hook; this instance is passed around
/// and is meant to be the _sole_ means of accessing global state.
/// 
/// WARNING 2: the `FizzleSingleton` prevents mutable aliasing within a single-threaded context, but
/// it cannot inherently prevent mutable access to data by multiple threads. Thread creation hooks
/// and scheduling routines need to ensure that any acquired `FizzGuard` instances are dropped prior
/// to another thread acquiring the global state.
pub unsafe fn fizzle_state_singleton() -> FizzleSingleton {
    FizzleSingleton {
        _phantom: Default::default(),
    }
}