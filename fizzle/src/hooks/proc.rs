//! Process creation shims.
//!
//!

use std::thread;

use crate::hook_macros;
use crate::state::identifiers::WorkerId;

hook_macros::hook! {
    unsafe fn fork() -> libc::pid_t => fizzle_fork(ctx) {

        let pid = hook_macros::real!(fork)();
        match pid {
            0 => {
                // Child process--fix all of the local state
                panic!("`fork` unimplemented");
            }
            1.. => {
                // Parent process--await execution

                let thread_id = thread::current().id();

                // This thread should still be able to execute afterwards
                ctx.add_ready_thread(thread_id);

                // This process should still be able to execute afterwards
                let process_id = ctx.local().process_id();

                ctx.global().mark_worker_ready(WorkerId {
                    process_id: process_id,
                    thread_id: thread_id,
                });

                // Pause our process until it gets delegated execution again.
                ctx.pause_current_process();
            }
            _ => () // else fork() returned -1 and failed--do nothing
        }

        pid
    }
}

hook_macros::hook! {
    unsafe fn exit(status: libc::c_int) => fizzle_exit(ctx) {
        if ctx.local().suspend_on_exit {
            // TODO: clean up any polling contexts here so that this process never gets
            // delegated to (other than for the purpose of running modules)

            // Temporary hack: whenever processes get delegated to here, just pass back to
            // another process (i.e. ignore inputs)
            loop {
                ctx.yield_thread()
            }
        } else {
            hook_macros::real!(exit)(status)
        }
    }
}

hook_macros::hook! {
    unsafe fn _exit(status: libc::c_int) => fizzle_exit2(ctx) {
        if ctx.local().suspend_on_exit {
            // TODO: clean up any polling contexts here so that this process never gets
            // delegated to (other than for the purpose of running modules)

            // Temporary hack: whenever processes get delegated to here, just pass back to
            // another process (i.e. ignore inputs)
            loop {
                ctx.yield_thread()
            }
        } else {
            hook_macros::real!(exit)(status)
        }
    }
}

// We need this to ensure that our `atexit` hook is called first when FIZZLE_NOEXIT is set.
hook_macros::hook! {
    unsafe fn atexit(cb: extern "C" fn()) => fizzle_atexit(_ctx) {
        hook_macros::real!(atexit)(cb)
    }
}
