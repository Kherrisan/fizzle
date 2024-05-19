//! Process creation shims.
//!
//!

use std::thread;

use crate::{hook_macros, state};

hook_macros::hook! {
    unsafe fn fork() -> libc::pid_t => fizzle_fork(ctx) {

        let pid = hook_macros::real!(fork)();
        if pid == 0 {
            // Child process--fix all of the local state

        } else if pid > 0 {
            // Parent process--await execution

            // This thread should still be able to execute afterwards
            ctx.local().ready_threads.push_back(thread::current().id());

            // This process should still be able to execute afterwards
            let process_id = ctx.local().process_id;
            ctx.global().mark_process_ready(process_id);

            // Pause our current thread until it gets delegated execution again.
            ctx.pause_current_process();
        }
        // else fork() returned -1 and failed--do nothing

        pid
    }
}
