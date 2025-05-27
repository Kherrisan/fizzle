//! Hooks for general functions that can be applied to any file descriptor.
//!

use crate::errno::Errno;
use crate::handlers::descriptor::{
    f_owner_ex, Descriptor, DescriptorCloseEvent, DescriptorDuplicateEvent, FcntlCommand,
    FcntlEvent,
};
use crate::scheduler::Scheduler;
use crate::{hook_macros, strace};
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("close(fd={}) -> ...", fd);
        match Scheduler::handle_event(&mut ctx, DescriptorCloseEvent::new(descriptor_id)) {
            Ok(()) => {
                crate::strace!("close(fd={}) -> 0", fd);
                0
            },
            Err(e) => {
                crate::strace!("close(fd={}) -> -1 ({})", fd, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn close_range(
        first: libc::c_uint,
        last: libc::c_uint,
        flags: libc::c_uint,
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close_range(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(first as i32);

        crate::strace!("close(fd={}) -> ...", fd);
        match Scheduler::handle_event(&mut ctx, DescriptorCloseEvent::new(descriptor_id)) {
            Ok(()) => {
                crate::strace!("close(fd={}) -> 0", fd);
                0
            },
            Err(e) => {
                crate::strace!("close(fd={}) -> -1 ({})", fd, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn dup(
        oldfd: libc::c_int
    ) -> libc::c_int => fizzle_dup(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(oldfd);

        crate::strace!("dup(oldfd={}) -> ...", oldfd);
        match Scheduler::handle_event(&mut ctx, DescriptorDuplicateEvent::new(descriptor_id, None, false)) {
            Ok(newfd) => {
                crate::strace!("dup(oldfd={}) -> {}", oldfd, newfd);
                newfd
            },
            Err(e) => {
                crate::strace!("dup(oldfd={}) -> -1 ({})", oldfd, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn dup2(
        oldfd: libc::c_int,
        newfd: libc::c_int
    ) -> libc::c_int => fizzle_dup2(ctx) {
        if oldfd == newfd {
            return newfd
        }

        let old_descriptor = Descriptor::from_raw_fd(oldfd);
        let new_descriptor = Descriptor::from_raw_fd(newfd);

        crate::strace!("dup2(oldfd={}, newfd={}) -> ...", oldfd, newfd);
        match Scheduler::handle_event(&mut ctx, DescriptorDuplicateEvent::new(old_descriptor, Some(new_descriptor), false)) {
            Ok(ret) => {
                crate::strace!("dup2(oldfd={}, newfd={}) -> {}", oldfd, newfd, ret);
                ret
            },
            Err(e) => {
                crate::strace!("dup2(oldfd={}, newfd={}) -> -1 ({})", oldfd, newfd, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn dup3(
        oldfd: libc::c_int,
        newfd: libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_dup3(ctx) {
        let close_on_exec = flags & libc::O_CLOEXEC > 0;

        let old_descriptor = Descriptor::from_raw_fd(oldfd);
        let new_descriptor = Descriptor::from_raw_fd(newfd);
        let flags_fmt = if close_on_exec {
            format!("O_CLOEXEC ({})", flags)
        } else {
            format!("{}", flags)
        };

        crate::strace!("dup3(oldfd={}, newfd={}, flags={}) -> ...", oldfd, newfd, flags_fmt);
        match Scheduler::handle_event(&mut ctx, DescriptorDuplicateEvent::new(old_descriptor, Some(new_descriptor), close_on_exec)) {
            Ok(ret) => {
                crate::strace!("dup3(oldfd={}, newfd={}, flags={}) -> {}", oldfd, newfd, flags_fmt, ret);
                ret
            },
            Err(e) => {
                crate::strace!("dup3(oldfd={}, newfd={}, flags={}) -> -1 ({})", oldfd, newfd, flags_fmt, e);
                e.set_errno();
                -1
            },
        }
    }
}

// TODO: refactor below functions to run within Scheduler

pub const F_SETSIG: libc::c_int = 10;
pub const F_GETSIG: libc::c_int = 11;

pub const F_SETOWN_EX: libc::c_int = 15;
pub const F_GETOWN_EX: libc::c_int = 16;

pub const F_GET_RW_HINT: libc::c_int = 1035;
pub const F_SET_RW_HINT: libc::c_int = 1036;
pub const F_GET_FILE_RW_HINT: libc::c_int = 1037;
pub const F_SET_FILE_RW_HINT: libc::c_int = 1038;

#[no_mangle]
pub unsafe extern "C" fn fcntl(fd: libc::c_int, cmd: libc::c_int, mut va_args: ...) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function fcntl() called within signal handler")
        }
    }
    
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        return match cmd {
            libc::F_DUPFD
            | libc::F_DUPFD_CLOEXEC
            | libc::F_SETFD
            | libc::F_SETFL
            | libc::F_SETOWN
            | F_SETSIG
            | libc::F_NOTIFY
            | libc::F_SETPIPE_SZ
            | libc::F_ADD_SEALS => {
                let arg: libc::c_int = va_args.arg();
                hook_macros::real_fcntl()(fd, cmd, arg)
            }
            libc::F_GETFD
            | libc::F_GETFL
            | libc::F_GETOWN
            | F_GETSIG
            | libc::F_GETLEASE
            | libc::F_GET_SEALS => hook_macros::real_fcntl()(fd, cmd),
            libc::F_SETLK
            | libc::F_SETLKW
            | libc::F_GETLK
            | libc::F_OFD_SETLK
            | libc::F_OFD_SETLKW
            | libc::F_OFD_GETLK
            | F_GETOWN_EX
            | F_SETOWN_EX
            | F_GET_RW_HINT
            | F_SET_RW_HINT
            | F_GET_FILE_RW_HINT
            | F_SET_FILE_RW_HINT => {
                let arg: *mut libc::c_void = va_args.arg();
                hook_macros::real_fcntl()(fd, cmd, arg)
            }
            _ => {
                Errno::EINVAL.set_errno();
                return -1;
            }
        };
    };

    crate::strace!("fcntl(fd={}, cmd={}, ...) -> ...", fd, cmd);

    let command = match cmd {
        libc::F_DUPFD => FcntlCommand::DupFd(va_args.arg()),
        libc::F_DUPFD_CLOEXEC => FcntlCommand::DupFdCloexec(va_args.arg()),
        libc::F_SETFD => FcntlCommand::SetFd(va_args.arg()),
        libc::F_SETFL => FcntlCommand::SetFl(va_args.arg()),
        libc::F_SETOWN => FcntlCommand::SetOwn(va_args.arg()),
        F_SETSIG => FcntlCommand::SetSig(va_args.arg()),
        libc::F_NOTIFY => FcntlCommand::Notify(va_args.arg()),
        libc::F_SETPIPE_SZ => FcntlCommand::SetPipeSize(va_args.arg()),
        libc::F_ADD_SEALS => FcntlCommand::AddSeals(va_args.arg()),
        libc::F_GETFD => FcntlCommand::GetFd,
        libc::F_GETFL => FcntlCommand::GetFl,
        libc::F_GETOWN => FcntlCommand::GetOwn,
        libc::F_GETLEASE => FcntlCommand::GetLease,
        libc::F_GET_SEALS => FcntlCommand::GetSeals,
        libc::F_SETLK => {
            FcntlCommand::SetLock(unsafe { &mut *(va_args.arg::<*mut libc::flock>()) })
        }
        libc::F_SETLKW => {
            FcntlCommand::SetLockWait(unsafe { &mut *(va_args.arg::<*mut libc::flock>()) })
        }
        libc::F_GETLK => {
            FcntlCommand::GetLock(unsafe { &mut *(va_args.arg::<*mut libc::flock>()) })
        }
        libc::F_OFD_SETLK => {
            FcntlCommand::SetLock(unsafe { &mut *(va_args.arg::<*mut libc::flock>()) })
        }
        libc::F_OFD_SETLKW => {
            FcntlCommand::SetLockWait(unsafe { &mut *(va_args.arg::<*mut libc::flock>()) })
        }
        libc::F_OFD_GETLK => {
            FcntlCommand::GetLock(unsafe { &mut *(va_args.arg::<*mut libc::flock>()) })
        }
        F_GETOWN_EX => FcntlCommand::GetOwnEx(unsafe { &mut *(va_args.arg::<*mut f_owner_ex>()) }),
        F_SETOWN_EX => FcntlCommand::SetOwnEx(unsafe { &mut *(va_args.arg::<*mut f_owner_ex>()) }),
        F_GET_RW_HINT => FcntlCommand::GetRwHint(unsafe { &mut *(va_args.arg::<*mut u64>()) }),
        F_SET_RW_HINT => FcntlCommand::SetRwHint(unsafe { &mut *(va_args.arg::<*mut u64>()) }),
        F_GET_FILE_RW_HINT => {
            FcntlCommand::GetFileRwHint(unsafe { &mut *(va_args.arg::<*mut u64>()) })
        }
        F_SET_FILE_RW_HINT => {
            FcntlCommand::SetFileRwHint(unsafe { &mut *(va_args.arg::<*mut u64>()) })
        }
        _ => {
            strace!("fcntl(fd={}, cmd={}, ...) -> -1 (EINVAL)", fd, cmd);
            Errno::EINVAL.set_errno();
            crate::state::set_entered_handler(false);
            return -1;
        }
    };

    match Scheduler::handle_event(
        &mut ctx,
        FcntlEvent::new(Descriptor::from_raw_fd(fd), command),
    ) {
        Ok(i) => {
            strace!("fcntl(fd={}, cmd={}, ...) -> {}", fd, cmd, i);
            drop(ctx);
            crate::hooks::post_hook();
            return i;
        }
        Err(e) => {
            strace!("fcntl(fd={}, cmd={}, ...) -> -1 ({})", fd, cmd, e);
            e.set_errno();
            drop(ctx);
            crate::hooks::post_hook();
            return -1;
        }
    }


}

// GNU libc unconditionally pulls a void* from va_args, so we should (hypothetically?) be okay doing this.
hook_macros::hook! {
    unsafe fn ioctl(
        fd: libc::c_int,
        request: libc::c_ulong,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_ioctl(_ctx) {
        log::info!("ioctl({}, {}, {})", fd, request, arg as usize);

        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        let res = libc::ioctl(fd, request, arg);
        unsafe {
            libc::close(fd);
        }

        res
    }
}
