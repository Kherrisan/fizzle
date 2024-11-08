use crate::errno::Errno;
use crate::handlers::file::AccessMode;
use crate::handlers::semaphore::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

use std::ffi::CStr;
use std::ptr;
use std::time::Duration;

hook_macros::hook! {
    unsafe fn sem_init(
        sem: *mut libc::sem_t,
        pshared: libc::c_int,
        value: libc::c_uint
    ) -> libc::c_int => fizzle_sem_init(ctx) {
        let pshared_bool = pshared != 0;
        let semaphore_id = SemaphorePtr::from(sem);

        crate::strace!("sem_init(sem={:?}, pshared={}, value={}) -> ...", sem, pshared, value);
        match Scheduler::handle_event(&mut ctx, SemInitEvent::new(semaphore_id, pshared_bool, value)) {
            Ok(()) => {
                crate::strace!("sem_init(sem={:?}, pshared={}, value={}) -> 0", sem, pshared, value);
                0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_open(
        name: *const libc::c_char,
        oflag: libc::c_int,
        mode: libc::mode_t,
        value: libc::c_uint
    ) -> *mut libc::sem_t => fizzle_sem_open(ctx) {

        let name = CStr::from_ptr(name);

        let Some(flags) = SemOpenFlags::from_bits(oflag) else {
            panic!("Unxepected oflags in sem_open()")
        };

        let create = if flags.contains(SemOpenFlags::CREATE) {
            let mode = AccessMode::from_bits(mode).unwrap();
            crate::strace!("sem_open(name={:?}, oflag={}, mode={}, value={}) -> ...", name, flags, mode, value);
            Some((mode, value))
        } else {
            crate::strace!("sem_open(name={:?}, oflag={}) -> ...", name, flags);
            None
        };

        match Scheduler::handle_event(&mut ctx, SemOpenEvent::new(name, flags.contains(SemOpenFlags::EXCLUSIVE), create)) {
            Ok(ret) => {
                crate::strace!("sem_open(name={:?}, oflag={}, ...) -> {:?}", name, oflag, ret);
                ret.to_mut_ptr()
            }
            Err(e) => {
                crate::strace!("sem_open(name={:?}, oflag={}, ...) -> NULL ({})", name, oflag, e);
                e.set_errno();
                ptr::null_mut()
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_destroy(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_destroy(ctx) {
        let sem_ptr = SemaphorePtr::from(sem);

        crate::strace!("sem_destroy(sem={:?}) -> ...", sem);

        match Scheduler::handle_event(&mut ctx, SemDestroyEvent::new(sem_ptr)) {
            Ok(()) => {
                crate::strace!("sem_destroy(sem={:?}) -> 0", sem);
                0
            }
            Err(e) => {
                crate::strace!("sem_destroy(sem={:?}) -> -1 ({})", sem, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_close(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_close(ctx) {
        let sem_ptr = SemaphorePtr::from(sem);

        crate::strace!("sem_close(sem={:?}) -> ...", sem);

        match Scheduler::handle_event(&mut ctx, SemCloseEvent::new(sem_ptr)) {
            Ok(()) => {
                crate::strace!("sem_close(sem={:?}) -> 0", sem);
                0
            }
            Err(e) => {
                crate::strace!("sem_close(sem={:?}) -> -1 ({})", sem, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_unlink(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_sem_unlink(ctx) {
        let name = CStr::from_ptr(path);

        crate::strace!("sem_unlink(path={:?}) -> ...", name);

        match Scheduler::handle_event(&mut ctx, SemUnlinkEvent::new(name)) {
            Ok(()) => {
                crate::strace!("sem_unlink(path={:?}) -> 0", name);
                0
            }
            Err(e) => {
                crate::strace!("sem_unlink(path={:?}) -> -1 ({})", name, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_post(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_post(ctx) {
        let sem_ptr = SemaphorePtr::from(sem);

        crate::strace!("sem_post(sem={:?}) -> ...", sem);

        match Scheduler::handle_event(&mut ctx, SemCloseEvent::new(sem_ptr)) {
            Ok(()) => {
                crate::strace!("sem_post(sem={:?}) -> 0", sem);
                0
            }
            Err(e) => {
                crate::strace!("sem_post(sem={:?}) -> -1 ({})", sem, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_wait(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_wait(ctx) {

        let semaphore_id = SemaphorePtr::from(sem);

        crate::strace!("sem_wait(sem={:?}) -> ...", semaphore_id);

        match Scheduler::handle_event(&mut ctx, SemWaitEvent::new(semaphore_id, WaitDuration::Indefinite)) {
            Ok(()) => {
                crate::strace!("sem_wait(sem={:?}) -> 0", semaphore_id);
                0
            },
            Err(e) => {
                crate::strace!("sem_wait(sem={:?}) -> -1 ({})", semaphore_id, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_trywait(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_trywait(ctx) {
        let semaphore_id = SemaphorePtr::from(sem);

        match Scheduler::handle_event(&mut ctx, SemWaitEvent::new(semaphore_id, WaitDuration::Immediate)) {
            Ok(()) => {
                crate::strace!("sem_trywait(sem={:?}) -> 0", semaphore_id);
                0
            },
            Err(e) => {
                crate::strace!("sem_trywait(sem={:?}) -> -1 ({})", semaphore_id, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_timedwait(
        sem: *mut libc::sem_t,
        abs_timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_sem_timedwait(ctx) {
        let semaphore_id = SemaphorePtr::from(sem);

        unsafe {
            if abs_timeout.is_null() || (*abs_timeout).tv_sec < 0 || (*abs_timeout).tv_nsec < 0 {
                crate::strace!("sem_timedwait(sem={:?}) -> -1 (EINVAL)", semaphore_id);
                Errno::EINVAL.set_errno();
                return -1
            }
        }

        let timeout = unsafe {
            Duration::from_secs((*abs_timeout).tv_sec as u64) + Duration::from_nanos((*abs_timeout).tv_nsec as u64)
        };

        match Scheduler::handle_event(&mut ctx, SemWaitEvent::new(semaphore_id, WaitDuration::Timed(timeout))) {
            Ok(()) => {
                crate::strace!("sem_timedwait(sem={:?}) -> 0", semaphore_id);
                0
            },
            Err(e) => {
                crate::strace!("sem_timedwait(sem={:?}) -> -1 ({})", semaphore_id, e);
                e.set_errno();
                -1
            },
        }
    }
}
