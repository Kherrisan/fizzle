use std::time::Duration;

use crate::hook_macros;
use crate::handlers::sleep::SleepEvent;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn sleep(
        seconds: libc::c_uint
    ) -> libc::c_uint => fizzle_sleep(ctx) {
        crate::strace!("sleep(seconds={}) -> ...", seconds);
        match Scheduler::handle_event(&mut ctx, SleepEvent::new(Duration::from_secs(seconds as u64))) {
            Ok(()) => {
                crate::strace!("sleep(seconds={}) -> 0", seconds);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn usleep(
        usec: libc::useconds_t
    ) -> libc::c_int => fizzle_usleep(ctx) {
        crate::strace!("usleep(usec={}) -> ...", usec);
        match Scheduler::handle_event(&mut ctx, SleepEvent::new(Duration::from_micros(usec as u64))) {
            Ok(()) => {
                crate::strace!("usleep(usec={}) -> 0", usec);
                0
            },
            Err(()) => unreachable!(),
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

        match Scheduler::handle_event(&mut ctx, SleepEvent::new(duration)) {
            Ok(()) => {
                if !rem.is_null() {
                    *rem = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                }
                crate::strace!("nanosleep(req={}.{}, rem={:?}) -> 0", sec, nsec, rem);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}
