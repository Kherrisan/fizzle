use crate::state;

use std::thread;

// All threads are always parked except for one
// Every time a thread is unparked, it immediately tries to acquire the fizzle state object.

/*
/// Marks the current thread as paused to the scheduler, executes `closure`, and then waits until
/// the scheduler has assigned control flow back to the current thread before continuing.
///
/// Note that yield_after may lead to instability across fuzzing runs unless the upper layer indicates
/// whether the blocking element should be waited for or not.
/// 
/// # Safety
/// 
/// Global state MUST NOT be locked at the time this function is called.
/// Likewise, global state MUST NOT be accessed within `closure`; otherwise, deadlock will most likely occur.
pub fn yield_after<F: FnOnce() -> ()>(blocking_fn: F) {
    let mut state = state::fizzle_state().lock().unwrap();

    // Delegate control to the next thread
    state.wake_next_thread();

    // Let go of state (to avoid deadlock)
    drop(state);

    // Runs concurrently with another thread
    blocking_fn();

    // TODO: there needs to be some way that certain higher-order actions can signal the fuzzer to wait (in case user wants to use passthrough mode)

    // Blocking action is finished--mark thread as now active again
    state::fizzle_state().lock().unwrap().ready_threads.push_back(thread::current().id());

    // Pause current thread until scheduler delegates execution to it
    thread::park();
    while !state::fizzle_state().lock().unwrap().thread_delegated() {
        thread::park();
    }

    // Continue regular execution--another thread has delegated execution back to our thread
}
*/

/// Marks the current thread as paused to the scheduler, then waits until the scheduler has
/// assigned control flow back to the current thread before continuing.
pub fn yield_thread() {
    let mut state = state::fizzle_state().lock().unwrap();

    // Delegate control back to the scheduler thread
    state.wake_next_thread();

    // Let go of state (to avoid deadlock)
    drop(state); 

    // Pause current thread until scheduler delegates execution to it
    thread::park();
    while !state::fizzle_state().lock().unwrap().thread_delegated() {
        thread::park();
    }

    // Continue regular execution--anothre thread has delegated execution back to our thread
}
