use crate::hook_macros;
use crate::state;

hook_macros::hook! {
    unsafe fn sleep(
        _seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_sleep(ctx) {
        // TODO: how do we handle timeouts in the general case?
        drop(ctx);
        state::FIZZLE_STATE.yield_thread();
        return 0
    }
}

hook_macros::hook! {
    unsafe fn usleep(
        _usec: libc::useconds_t
    ) -> libc::c_int => fizzle_usleep(ctx) {
        // TODO: how do we handle timeouts in the general case?
        drop(ctx);
        state::FIZZLE_STATE.yield_thread();
        return 0
    }
}
