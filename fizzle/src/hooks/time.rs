use crate::handlers::time::GetTimeEvent;
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn time(
        tloc: *mut libc::time_t
    ) -> libc::time_t => fizzle_time(ctx) {

        crate::strace!("time(tloc={:?}) -> ...", tloc);

        match Scheduler::handle_event(&mut ctx, GetTimeEvent) {
            Ok(duration) => {
                let nsecs = duration.as_secs() as libc::time_t;

                if let Some(tloc_mut) = tloc.as_mut() {
                    *tloc_mut = nsecs;
                }

                crate::strace!("time(tloc={:?}) -> {}", tloc, nsecs);
                nsecs
            },
            Err(e) => {
                crate::strace!("time(tloc={:?}) -> -1 ({})", tloc, e);
                -1
            },
        }
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
        clockid: libc::clockid_t,
        tp: *mut libc::timespec
    ) -> libc::c_int => fizzle_clock_gettime(ctx) {

        crate::strace!("clock_gettime(clockid={}, tp={:?}) -> ...", clockid, tp);

        // TODO: handle different clocks specially
        match Scheduler::handle_event(&mut ctx, GetTimeEvent) {
            Ok(duration) => {
                if let Some(tp_mut) = tp.as_mut() {
                    *tp_mut = libc::timespec {
                        tv_sec: duration.as_secs() as i64,
                        tv_nsec: duration.subsec_nanos() as i64,
                    };
                };

                crate::strace!("clock_gettime(clockid={}, tp={:?}) -> 0", clockid, tp);
                0
            },
            Err(e) => {
                crate::strace!("clock_gettime(clockid={}, tp={:?}) -> -1 ({})", clockid, tp, e);
                -1
            },
        }
    }
}

// TODO: interpose `daylight`

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
        tv: *mut libc::timeval,
        tz: *mut libc::timezone
    ) -> libc::time_t => fizzle_gettimeofday(ctx) {
        crate::strace!("gettimeofday(tv={:?}, tz={:?}) -> ...", tv, tz);

        match Scheduler::handle_event(&mut ctx, GetTimeEvent) {
            Ok(duration) => {
                if let Some(tv_mut) = tv.as_mut() {
                    *tv_mut = libc::timeval {
                        tv_sec: duration.as_secs() as i64,
                        tv_usec: duration.subsec_micros() as i64,
                    };
                };

                crate::strace!("gettimeofday(tv={:?}, tz={:?}) -> 0", tv, tz);
                0
            },
            Err(e) => {
                crate::strace!("gettimeofday(tv={:?}, tz={:?}) -> -1 ({})", tv, tz, e);
                -1
            },
        }
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
