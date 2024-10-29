
use std::thread;

use crate::state;
use crate::state::{FizzleSingleton, FizzleState, ReadyInfo, WorkerId};

// Input parameters are contained within the event
pub trait Event {
    /// The type returned by the hook function.
    type Out;
    /// The Success type associated with the event.
    type S: Success<Out = Self::Out>;
    /// The Error type associated with the event.
    type E: Error<Out = Self::Out>;

    /// The next action associated with an event.
    fn next_action(&mut self) -> Option<fn(&mut Self, &mut FizzleState) -> Outcome<Self::S, Self::E>>;
}

pub enum Outcome<S: Success, E: Error> {
    /// The value S should be returned for the hook function.
    Success(S),
    /// The error value and errno specified in E should be returned for the function.
    Error(E),
    /// The event should move on to its next action after yielding.
    Yield,
    /// The event should move on to its next action immediately.
    Continue,
}

pub trait Success {
    type Out;

    /// Returns the success value used by the function.
    fn ret(self) -> Self::Out;
}

pub trait Error {
    type Out;

    /// Implements functionality used for [`ret()`](Error::ret). This function is not meant to be
    /// called directly; use `ret()` instead.
    fn ret_impl(&self) -> (i32, Self::Out);

    /// Sets the `errno` value appropriately and returns an error value.
    /// 
    /// This method is meant to be called at the end of a method hook.
    fn ret(&self) -> Self::Out {
        let (errno, out) = Self::ret_impl(self);
        unsafe {
            *libc::__errno_location() = errno;
        }

        out
    }
}

pub struct Scheduler;

impl Scheduler {
    pub fn handle_event<E: Event>(mut event: E) -> Result<E::S, E::E> {
        while let Some(action) = event.next_action() {
            // pre-actions here

            // TODO: handle received signals and thread cancellation events here (but avoid recursion!!)

            // 1. Is this thread scheduled to be cancelled? (if so, cancel now)

            // 2. Does this thread have an outstanding signal? (if so, run handler now)

            let mut ctx = unsafe { state::fizzle_singleton() };
            let mut state = ctx.acquire();
            match action(&mut event, &mut state) {
                Outcome::Success(s) => return Ok(s),
                Outcome::Error(e) => return Err(e),
                Outcome::Continue => (),
                // TODO: any other useful outcomes? Outcome::Cancel?
                Outcome::Yield => {
                    log::debug!("Thread being yielded");
                    drop(state);

                    'delegate: loop {
                        let mut state = ctx.acquire();
                        let worker = Self::next_ready_worker(&mut state);

                        if let Some(worker_id) = worker {
                            log::debug!("Scheduling worker {:?} for execution", worker_id);

                            // Give the next process the info it needs to run the correct thread
                            state.global.waking_id = Some(worker_id.thread_id);
                            let local_process_id = state.local.process_id;
                            drop(state);

                            if worker_id.process_id != local_process_id {
                                // Execution needs to move to another process

                                ctx.wake_process(worker_id.process_id);
                                // SAFETY: `state` is not being held here
                                ctx.pause_current_process();
                            }

                            'awaken: loop {
                                // This process/thread has been awakened...
                                let mut state = ctx.acquire();
                                if let Some(thread_id) = state.global.waking_id.take() {
                                    drop(state);
                                    // ...because it has a thread ready to be executed
                                    if thread::current().id() != thread_id {
                                        // It's some thread other than this one--schedule accordingly

                                        ctx.thread_lock(&thread_id).as_ref().unwrap().post();
                                        // SAFETY: `state` is not being held here
                                        ctx.pause_current_thread();
                                    } else {
                                        // It's this thread--break out of the delegation loop
                                        break 'delegate
                                    }
                                } else {
                                    // ...because it needs to assist another process/thread.
                                    // This may be for the following reasons:
                                    if let Some(_worker_id) = state.global.exiting_id.take() {
                                        // 1. A thread is being reaped and needs to pass execution on.

                                        // TODO: use `pthread_join` or `waitpid` here to ensure completion?

                                        // Now this thread is in charge, so it needs to delegate execution:
                                        break 'awaken
                                    } else {
                                        // 2. A thread in the primary process needs to run plugins.
                                    }
                                }
                            }
                        } else {
                            log::debug!("No workers were ready for execution");

                            // difference between 'awaken' and 'schedule'

                            // TODO: 1. shift to main process
                            // 2. Run plugins
                            // 3. If no change, run completion routine 
                        }
                    }

                    
                    // TODO: handle yielding

                    // Yield loop:

                    // 1. Does another thread need help with cancelling?

                    // 2. Does this thread have an outstanding signal?

                    // 3. Are there no new threads/processes to schedule?

                    // 3a. Have plugins all returned None?
                    // 3b. Transition to next phase of fuzzing
                }
            }
        }

        // An Event's final action should always lead to `Outcome::Success` or `Outcome::Error`
        panic!("Event ran out of actions to take")
    }

    fn next_ready_worker(state: &mut FizzleState) -> Option<WorkerId> {
        while let Some(item) = state.global.ready.pop_front() {
            match item {
                ReadyInfo::Worker(worker_id) => {
                    // new_raised_events will be empty here
                    return Some(worker_id);
                }
                ReadyInfo::Poller(poller_id) => {
                    log::trace!(
                        "Checking if poller {:?} is ready for execution...",
                        poller_id
                    );
                    let global = &mut state.global;

                    let poller_info = global.pollers.get_mut(&poller_id).unwrap();
                    
                    for polled_id in poller_info.raised_events.iter() {
                        let polled_info = global.polled_events.get_mut(&polled_id).unwrap();
                        if polled_info.event_raised {
                            log::trace!("Poller {:?} is ready for execution", poller_id);
                            return Some(poller_info.worker_id);
                        }
                    }

                    log::trace!("Poller {:?} is not ready for execution--clearing events", poller_id);
                    poller_info.raised_events.clear();
                }
            }
        }

        return None
    }

    fn run_plugins() {

    }

    /// Terminates the current thread, cleaning up its resources along the way.
    fn terminate_thread(ctx: &mut FizzleSingleton, method: TerminationMethod) -> ! {
        log::info!("Thread being terminated...");

        let thread_id = thread::current().id();
        let mut state = ctx.acquire();

        let mut cleanup_routines =state 
            .local
            .pthread_cleanup
            .remove(&thread_id)
            .unwrap_or_default();

        // Remove and gather cleanup routines for pthread keys
        let pthread_keys: Vec<u32> = state.local.pthread_keys.keys().copied().collect();
        for key in pthread_keys {
            if let Some(values) = state.local.pthread_key_values.get_mut(&key) {
                if let Some(p) = values.remove(&thread_id) {
                    let mut destructor = *state.local.pthread_keys.get(&key).unwrap();
                    destructor.arg = Some(p);
                    cleanup_routines.push_back(destructor);
                }
            }
        }

        drop(state);

        // Run cleanup routines, hooking any functions within the routines
        Self::run_outside_hook(|| {
            for routine in cleanup_routines {
                routine.call();
            }
        });

        // Clean up this thread's semaphore
        ctx.destroy_thread_lock();

        let mut state = ctx.acquire();

        // Mark this thread as dead for future threads that may wait on it.
        state.local.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = state.local.awaiting_thread_death.remove(&thread_id) {
            let process_id = state.local.process_id;
            for thread_id in awaiting_threads {
                state.global
                    .ready
                    .push_back(ReadyInfo::Worker(WorkerId {
                        process_id,
                        thread_id,
                    }))
                    .unwrap();
            }
        }

        // Clean up local state of thread
        state.local.pthread_cleanup.remove(&thread_id);
        state.local.signals.remove(&thread_id);
        state.local.pthreads.remove(&unsafe { libc::pthread_self() });

        // Delegate execution to...
        if let Some(thread_id) = state.local.pthreads.values().next().cloned() {
            // ...another running thread in this process
            drop(state);
            ctx.thread_lock(&thread_id).as_ref().unwrap().post();

        } else {
            // ...another process, as this process is going out of scope.
            drop(state);

            // Free process resources
            Scheduler::cleanup_process(ctx);
            let mut state = ctx.acquire();

            // Choose another process to delegate execution to
            let Some(delegate_process_id) = state.global.process_signals.keys().next() else {
                panic!("Last process/thread terminated (graceful crash)");
            };

            drop(state);
            ctx.wake_process(delegate_process_id);
        }

        // TODO: What about when the main thread exits? Needs atexit() handler installed when process first created...

        // =======================DANGER ZONE: CONCURRENCY===========================
        //       **ctx.acquire() must not be called from this point onwards**

        // Now either cancel or signal the current thread to cause it to exit so that threads
        // waiting on `join()` will properly reap threads (and avoid zombies)
        match method {
            TerminationMethod::Cancellation => unsafe {
                libc::pthread_cancel(libc::pthread_self());
                libc::sleep(1); // Acts as a backup cancellation point in case `pthread_cancel` didn't work
                panic!("`pthread_cancel` failed to kill current thread");
            },
            TerminationMethod::Exit(retval) => unsafe { libc::pthread_exit(retval) },
            TerminationMethod::SigTerm => unsafe {
                libc::pthread_kill(libc::pthread_self(), libc::SIGTERM);
                libc::sleep(1); // Acts as a backup cancellation point in case `pthread_kill` didn't work
                panic!("`pthread_kill` failed to kill current thread");
            },
        }
    }

    /// Removes any global state associated with the given process.
    fn cleanup_process(ctx: &mut FizzleSingleton) {
        // TODO: remove all active file descriptors, handles from local state so they're freed from global

        // Clean up process state
        let mut state = ctx.acquire();
        let process_id = state.local.process_id;
        state.global.process_signals.downref(&process_id);

        // Clean up this process's semaphore
        state.global.process_locks[usize::from(process_id)] = None;
    }

    /// Runs the given routine outside of the context of the current method hook.
    /// 
    /// Any system library calls performed by code within this closure will be hooked and handled
    /// as if it were being run by the program.
    fn run_outside_hook<F, R>(f: F) -> R
    where
        F: FnOnce() -> R
    {
        debug_assert!(state::has_entered_handler());
        state::set_entered_handler(false);
        let ret = f();
        state::set_entered_handler(true);
        ret
    }


}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminationMethod {
    Cancellation,
    Exit(*mut libc::c_void),
    #[allow(unused)]
    SigTerm, // TODO: implement
}
