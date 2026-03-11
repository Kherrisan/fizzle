use std::time::Duration;

use crate::errno::Errno;
use crate::handlers::time::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;
use crate::state::TimerType;

union union_sigval {
    sival_int: libc::c_int,
    sival_ptr: *mut libc::c_void,
}

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
        clockid: libc::clockid_t,
        rvp: *mut libc::sigevent,
        timerid: *mut libc::timer_t
    ) -> libc::c_int => fizzle_timer_create(ctx) {
        crate::strace!("timer_create(clockid={}, rvp={:?}, value={:?}) -> ...", clockid, rvp, timerid);

        let mut signal_number: Option<libc::c_int> = None;

        let mut rvp_ptr = rvp;

        if (rvp_ptr.is_null()) {
            // Because the `libc` library is incomplete, we need to do this
            // in 2 steps. First initialize struct with zero memory.
            let rvp_layout = std::alloc::Layout::new::<libc::sigevent>();
            rvp_ptr = std::alloc::alloc(rvp_layout) as *mut libc::sigevent;
            if (rvp_ptr.is_null()) {
                std::alloc::handle_alloc_error(rvp_layout);
            }
            *rvp_ptr = std::mem::zeroed();
            // Then, we set each field as needed.
            (*rvp_ptr).sigev_notify = libc::SIGEV_SIGNAL;
            (*rvp_ptr).sigev_signo =  libc::SIGALRM;
            (*rvp_ptr).sigev_value = libc::sigval { sival_ptr: *timerid };
        }
        if (&(*rvp_ptr).sigev_notify == &libc::SIGEV_SIGNAL) {
            // sival_ptr is (presumably) a pointer that points to the signal value. Need to test.
            signal_number = Some((*rvp_ptr).sigev_signo);
        }
        else {
        }
        
        match Scheduler::handle_event(&mut ctx, TimerCreateEvent::new(clockid, signal_number)) {
            Ok(timer_id_val) => {
                if let Some(val_mut) = timerid.as_mut() {
                    *val_mut = timer_id_val as *mut libc::timer_t as *mut libc::c_void;
                };

                crate::strace!("timer_create(clockid={}, rvp={:?}, value={:?}) -> 0", clockid, rvp, timerid);
                0
            },
            Err(e) => {
                crate::strace!("timer_create(clockid={}, rvp={:?}, value={:?}) -> -1 ({:?})", clockid, rvp, timerid, e);
                -1
            },
        }
    }
}
hook_macros::hook! {
    unsafe fn timer_delete(

    ) -> libc::time_t => fizzle_timer_delete(_ctx) {
        unimplemented!("timer_delete()")
    }
}

hook_macros::hook! {
    unsafe fn timer_getoverrun(

    ) -> libc::time_t => fizzle_timer_getoverrun(_ctx) {
        unimplemented!("timer_getoverrun()")
    }
}

hook_macros::hook! {
    unsafe fn timer_gettime(
        timerid: libc::timer_t,
        value: *mut libc::itimerspec
    ) -> libc::c_int => fizzle_timer_gettime(ctx) {
        crate::strace!("timer_gettime(timerid={:?}, value={:?}) -> ...", timerid, value);
        
        /*
        match Scheduler::handle_event(&mut ctx, GetItimerEvent::new(which_enum)) {
            Ok(timer_val) => {
                if let Some(val_mut) = curr_value.as_mut() {
                    *val_mut = libc::itimerval {
                        it_interval: libc::timeval {
                            tv_sec: timer_val.interval.as_secs() as i64,
                            tv_usec: timer_val.interval.subsec_micros() as i64,
                        },
                        it_value: libc::timeval {
                            tv_sec: timer_val.val.as_secs() as i64,
                            tv_usec: timer_val.val.subsec_micros() as i64,
                        }
                    };
                };

                crate::strace!("timer_gettime(timerid={:?}, value={:?}) -> 0", timerid, value);
                0
            },
            Err(e) => {
                crate::strace!("timer_gettime(timerid={:?}, value={:?}) -> -1 ({})", timerid, value, e);
                -1
            },
        }
        */
        0
    }
}

hook_macros::hook! {
    unsafe fn timer_settime(
        timerid: libc::timer_t,
        flags: libc::c_int,
        value: *mut libc::itimerspec,
        ovalue: *mut libc::itimerspec
    ) -> libc::time_t => fizzle_time_settime(_ctx) {

        unimplemented!("timer_settime()")
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
        clockid: libc::clockid_t,
        res: *mut libc::timespec
    ) -> libc::c_int => fizzle_clock_getres(_ctx) {
        log::warn!("unimplemented: clock_getres()");
        libc::clock_getres(clockid, res)
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
    unsafe fn getcpu(
        _cpu: *mut libc::c_uint,
        _node: *mut libc::c_uint,
        _tcache: *mut libc::c_void // struct getcpu_cache
    ) -> libc::c_int => fizzle_getcpu(_ctx) {
        unimplemented!("getcpu()")
    }
}

hook_macros::hook! {
    unsafe fn ftime(
        _tp: *mut libc::c_void // struct timeb
    ) -> libc::c_int => fizzle_ftime(_ctx) {
        unimplemented!("ftime()")
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
     unsafe fn times(
        tms: *mut libc::tms
    ) -> libc::clock_t => fizzle_times(ctx) {
        crate::strace!("times(tms={tms:?}) -> ...");

        match Scheduler::handle_event(&mut ctx, GetTimesEvent) {
            Ok(t) => {
                *tms = t;
                crate::strace!("times(tms={tms:?}) -> 0");
                0
            },
            Err(e) => {
                crate::strace!("times(tms={tms:?}) -> -1 ({e})");
                e.set_errno();
                -1
            },
        }
    }
}

/*
hook_macros::hook! {
     unsafe fn getrusage(
        who: libc::c_int,
        usage: *mut libc::rusage
    ) -> libc::clock_t => fizzle_getrusage(_ctx) {
        unimplemented!("getrusage()")
    }
}
*/

hook_macros::hook! {
    unsafe fn alarm(
        seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_alarm(ctx) {
        crate::strace!("alarm(seconds={}) -> ...", seconds);

        // TODO: verify correctness of itimerval values

        let new_value = if seconds > 0 {
            Some(ItimerValue { interval: Duration::ZERO, val: Duration::from_secs(seconds as u64) })
        } else {
            None
        };

        match Scheduler::handle_event(&mut ctx, SetItimerEvent::new(TimerType::Real, new_value)) {
            Ok(old_value) => {
                let remaining_secs = old_value.val.as_secs();
                crate::strace!("alarm(seconds={}) -> {}", seconds, remaining_secs);
                remaining_secs.try_into().unwrap()
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn setitimer(
        which: libc::c_int,
        new_value: *mut libc::itimerval,
        old_value: *mut libc::itimerval
    ) -> libc::c_int => fizzle_setitimer(ctx) {
        crate::strace!("setitimer(which={}, new_value={:?}, old_value={:?}) -> ...", which, new_value, old_value);

        let which_enum = match which {
            libc::ITIMER_REAL => TimerType::Real,
            libc::ITIMER_VIRTUAL => TimerType::Virtual,
            libc::ITIMER_PROF => TimerType::Prof,
            _ => {
                crate::strace!("setitimer(which={}, new_value={:?}, old_value={:?}) -> -1 (EINVAL)", which, new_value, old_value);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        // TODO: verify correctness of itimerval values

        let new = new_value.as_mut().map(|n| ItimerValue {
            interval: Duration::from_secs(n.it_interval.tv_sec as u64) + Duration::from_micros(n.it_interval.tv_usec as u64),
            val: Duration::from_secs(n.it_value.tv_sec as u64) + Duration::from_micros(n.it_value.tv_usec as u64),
        });

        match Scheduler::handle_event(&mut ctx, SetItimerEvent::new(which_enum, new)) {
            Ok(timer_val) => {
                if let Some(val_mut) = old_value.as_mut() {
                    *val_mut = libc::itimerval {
                        it_interval: libc::timeval {
                            tv_sec: timer_val.interval.as_secs() as i64,
                            tv_usec: timer_val.interval.subsec_micros() as i64,
                        },
                        it_value: libc::timeval {
                            tv_sec: timer_val.val.as_secs() as i64,
                            tv_usec: timer_val.val.subsec_micros() as i64,
                        }
                    };
                };

                crate::strace!("setitimer(which={}, new_value={:?}, old_value={:?}) -> 0", which, new_value, old_value);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn getitimer(
        which: libc::c_int,
        curr_value: *mut libc::itimerval
    ) -> libc::c_int => fizzle_getitimer(ctx) {
        crate::strace!("getitimer(which={}, curr_value={:?}) -> ...", which, curr_value);

        let which_enum = match which {
            libc::ITIMER_REAL => TimerType::Real,
            libc::ITIMER_VIRTUAL => TimerType::Virtual,
            libc::ITIMER_PROF => TimerType::Prof,
            _ => {
                crate::strace!("getitimer(which={}, curr_value={:?}) -> -1 (EINVAL)", which, curr_value);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, GetItimerEvent::new(which_enum)) {
            Ok(timer_val) => {
                if let Some(val_mut) = curr_value.as_mut() {
                    *val_mut = libc::itimerval {
                        it_interval: libc::timeval {
                            tv_sec: timer_val.interval.as_secs() as i64,
                            tv_usec: timer_val.interval.subsec_micros() as i64,
                        },
                        it_value: libc::timeval {
                            tv_sec: timer_val.val.as_secs() as i64,
                            tv_usec: timer_val.val.subsec_micros() as i64,
                        }
                    };
                };

                crate::strace!("getitimer(which={}, curr_value={:?}) -> 0", which, curr_value);
                0
            },
            Err(e) => {
                crate::strace!("getitimer(which={}, curr_value={:?}) -> -1 ({})", which, curr_value, e);
                -1
            },
        }
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
