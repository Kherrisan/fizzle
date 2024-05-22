//! Process creation shims.
//!
//!

use std::thread;

use crate::hook_macros;
use crate::state::WorkerId;

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
                ctx.local().ready_threads.push_back(thread_id);

                // This process should still be able to execute afterwards
                let process_id = ctx.local().process_id();

                ctx.global().mark_worker_ready(WorkerId {
                    process: process_id,
                    thread: thread_id,
                });

                // Pause our process until it gets delegated execution again.
                ctx.pause_current_process();
            }
            _ => () // else fork() returned -1 and failed--do nothing
        }

        pid
    }
}
