use std::time::Duration;

use crate::handlers::sleep::SleepEvent;
use crate::hook_macros;
use crate::scheduler::Scheduler;
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn pause() -> libc::c_int => fizzle_pause(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function pause() called within signal handler")
            }
        }

        crate::strace!("pause() -> ...");
        match Scheduler::handle_event(&mut ctx, SleepEvent::new(None)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("pause() -> -1 ({})", e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sleep(
        seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_sleep(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function sleep() called within signal handler")
            }
        }

        crate::strace!("sleep(seconds={}) -> ...", seconds);
        match Scheduler::handle_event(&mut ctx, SleepEvent::new(Some(Duration::from_secs(seconds as u64)))) {
            Ok(()) => {
                crate::strace!("sleep(seconds={}) -> 0", seconds);
                0
            },
            Err(e) => {
                crate::strace!("sleep(seconds={}) -> -1 ({})", seconds, e);
                e.set_errno();
                seconds // TODO: should be number of seconds left to sleep
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn usleep(
        usec: libc::useconds_t
    ) -> libc::c_int => fizzle_usleep(ctx) {
        crate::strace!("usleep(usec={}) -> ...", usec);
        match Scheduler::handle_event(&mut ctx, SleepEvent::new(Some(Duration::from_micros(usec as u64)))) {
            Ok(()) => {
                crate::strace!("usleep(usec={}) -> 0", usec);
                0
            },
            Err(e) => {
                crate::strace!("usleep(usec={}) -> -1 ({})", usec, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn nanosleep(
        req: *const libc::timespec,
        rem: *mut libc::timespec
    ) -> libc::c_int => fizzle_nanosleep(ctx) {

        let sec = (*req).tv_sec;
        let nsec = (*req).tv_nsec;
        let duration = Duration::from_secs(sec as u64) + Duration::from_nanos(nsec as u64);

        crate::strace!("nanosleep(req={}.{}, rem={:?}) -> ...", sec, nsec, rem);

        match Scheduler::handle_event(&mut ctx, SleepEvent::new(Some(duration))) {
            Ok(()) => {
                if !rem.is_null() {
                    *rem = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                }
                crate::strace!("nanosleep(req={:?}, rem={:?}) -> 0", duration, rem);
                0
            },
            Err(e) => {
                crate::strace!("nanosleep(req={:?}, rem={:?}) -> -1 ({})", duration, rem, e);
                e.set_errno();
                -1
            },
        }
    }
}
