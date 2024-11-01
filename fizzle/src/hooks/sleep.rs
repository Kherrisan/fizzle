use std::thread;

use crate::hook_macros;

hook_macros::hook! {
    unsafe fn sleep(
        seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_sleep(ctx) {
        // TODO: how do we handle timeouts in the general case?

        if seconds <= 1 {
            let mut state = ctx.acquire();
            state.mark_thread_ready(thread::current().id()); // TODO: mark delayed ready??
            drop(state);
        }

        ctx.yield_thread();

        0
    }
}

hook_macros::hook! {
    unsafe fn usleep(
        usec: libc::useconds_t
    ) -> libc::c_int => fizzle_usleep(ctx) {
        // TODO: how do we handle timeouts in the general case?

        if usec <= 1_000_000 {
            let mut state = ctx.acquire();
            state.mark_thread_ready(thread::current().id()); // TODO: mark delayed ready??
            drop(state);
        }

        ctx.yield_thread();

        0
    }
}

hook_macros::hook! {
    unsafe fn nanosleep(
        req: *const libc::timespec,
        rem: *mut libc::timespec
    ) -> libc::c_int => fizzle_nanosleep(ctx) {
        log::info!("nanosleep({}.{})", (*req).tv_sec, (*req).tv_nsec);

        if (*req).tv_sec == 1 && (*req).tv_nsec == 0 || (*req).tv_nsec <= 1_000_000_000 {
            let mut state = ctx.acquire();
            state.mark_thread_ready(thread::current().id()); // TODO: mark delayed ready??
            drop(state);
        }

        ctx.yield_thread();

        if !rem.is_null() {
            (*rem).tv_sec = 0;
            (*rem).tv_nsec = 0;
        }

        0
    }
}
