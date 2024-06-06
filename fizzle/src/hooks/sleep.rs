use crate::hook_macros;

hook_macros::hook! {
    unsafe fn sleep(
        seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_sleep(ctx) {
        // TODO: how do we handle timeouts in the general case?
        ctx.yield_thread();
        return 0
    }
}

hook_macros::hook! {
    unsafe fn usleep(
        usec: libc::useconds_t
    ) -> libc::c_int => fizzle_usleep(ctx) {
        // TODO: how do we handle timeouts in the general case?
        ctx.yield_thread();
        return 0
    }
}
