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
    unsafe fn timer_create(

    ) -> libc::time_t => fizzle_adjtime(_ctx) {
        unimplemented!("adjtime()")
    }
}

hook_macros::hook! {
    unsafe fn timerfd_create(
        _clockid: libc::c_int,
        _flags: libc::c_int
    ) -> libc::c_int => fizzle_timerfd_create(_ctx) {
        unimplemented!("timerfd_create()")
    }
}

hook_macros::hook! {
    unsafe fn timerfd_settime(
        _fd: libc::c_int,
        _new_value: *const libc::itimerspec,
        _old_value: *mut libc::itimerspec
    ) -> libc::c_int => fizzle_timerfd_settime(_ctx) {
        unimplemented!("timerfd_settime()")
    }
}

hook_macros::hook! {
    unsafe fn timerfd_gettime(
        _fd: libc::c_int,
        _curr_value: *mut libc::itimerspec
    ) -> libc::c_int => fizzle_timerfd_gettime(_ctx) {
        unimplemented!("timerfd_gettime()")
    }
}

hook_macros::hook! {
    unsafe fn clock_getres(
        _clockid: libc::clockid_t,
        _res: *mut libc::timespec
    ) -> libc::c_int => fizzle_clock_getres(_ctx) {
        unimplemented!("clock_getres()")
    }
}

hook_macros::hook! {
    unsafe fn clock_gettime(
        _clockid: libc::clockid_t,
        _tp: *mut libc::timespec
    ) -> libc::c_int => fizzle_clock_gettime(_ctx) {
        unimplemented!("clock_gettime()")
    }
}

hook_macros::hook! {
    unsafe fn clock_settime(
        _clockid: libc::clockid_t,
        _tp: *const libc::timespec
    ) -> libc::c_int => fizzle_clock_settime(_ctx) {
        unimplemented!("clock_settime()")
    }
}

hook_macros::hook! {
    unsafe fn clock_getcpuclockid(
        _pid: libc::pid_t,
        _clockid: *mut libc::clockid_t
    ) -> libc::c_int => fizzle_clock_getcpuclockid(_ctx) {
        unimplemented!("clock_getcpuclockid()")
    }
}

hook_macros::hook! {
    unsafe fn gettimeofday(
        _tv: *mut libc::timeval,
        _tz: *mut libc::timezone
    ) -> libc::time_t => fizzle_gettimeofday(_ctx) {
        unimplemented!("gettimeofday()")
    }
}

hook_macros::hook! {
    unsafe fn settimeofday(
        _tv: *const libc::timeval,
        _tz: *const libc::timezone
    ) -> libc::time_t => fizzle_settimeofday(_ctx) {
        unimplemented!("settimeofday()")
    }
}

hook_macros::hook! {
    unsafe fn alarm(
        _seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_alarm(_ctx) {
        unimplemented!("alarm()");
    }
}

hook_macros::hook! {
    unsafe fn setitimer(
        _which: libc::c_int,
        _new_value: *mut libc::itimerval,
        _old_value: *mut libc::itimerval
    ) -> libc::c_int => fizzle_setitimer(_ctx) {
        unimplemented!("setitimer()");
    }
}

hook_macros::hook! {
    unsafe fn getitimer(
        _which: libc::c_int,
        _curr_value: *mut libc::itimerval
    ) -> libc::c_int => fizzle_getitimer(_ctx) {
        unimplemented!("getitimer()");
    }
}

hook_macros::hook! {
    unsafe fn adjtimex(
        _buf: *mut libc::timex
    ) -> libc::c_int => fizzle_adjtimex(_ctx) {
        unimplemented!("adjtimex()");
    }
}

hook_macros::hook! {
    unsafe fn clock_adjtime(
        _clk_id: libc::clockid_t,
        _buf: *mut libc::timex
    ) -> libc::c_int => fizzle_clock_adjtime(_ctx) {
        unimplemented!("clock_adjtime()");
    }
}

hook_macros::hook! {
    unsafe fn ntp_adjtime(
        _buf: *mut libc::timex
    ) -> libc::c_int => fizzle_ntp_adjtime(_ctx) {
        unimplemented!("ntp_adjtime()");
    }
}
