use crate::hook_macros;

hook_macros::hook! {
    unsafe fn time(
        tloc: *mut libc::time_t
    ) -> libc::time_t => fizzle_time(_ctx) {
        if !tloc.is_null() {
            *tloc = 1500000000;
        }

        1500000000
    }
}

hook_macros::hook! {
    unsafe fn timerfd_create(
        _clockid: libc::c_int,
        _flags: libc::c_int
    ) => fizzle_timerfd_create(_ctx) {
        unimplemented!("timerfd_create()")
    }
}
