use crate::hook_macros;

hook_macros::hook! {
    unsafe fn sleep(
        _seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_sleep(_ctx) {
        // TODO: how do we handle timeouts in the general case?
        return 0
    }
}

hook_macros::hook! {
    unsafe fn usleep(
        _usec: libc::useconds_t
    ) -> libc::c_int => fizzle_usleep(_ctx) {
        // TODO: how do we handle timeouts in the general case?
        return 0
    }
}
