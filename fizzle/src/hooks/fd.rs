//! Hooks for general functions that can be applied to any file descriptor.
//!

use crate::handlers::descriptor::{DescriptorCloseEvent, DescriptorDuplicateEvent, DescriptorId, DescriptorInfo};
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close(ctx) {
        let descriptor_id = DescriptorId::from_raw_fd(fd);

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
        let descriptor_id = DescriptorId::from_raw_fd(oldfd);

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

        let old_descriptor = DescriptorId::from_raw_fd(oldfd);
        let new_descriptor = DescriptorId::from_raw_fd(newfd);

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

        let old_descriptor = DescriptorId::from_raw_fd(oldfd);
        let new_descriptor = DescriptorId::from_raw_fd(newfd);
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

// GNU libc unconditionally pulls a void* from va_args, so we should (hypothetically?) be okay doing this.
hook_macros::hook! {
    unsafe fn fcntl(
        fd: libc::c_int,
        cmd: libc::c_int,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_fcntl(ctx) {
        let mut state = ctx.acquire();

        match state.local.fds.get_mut(&DescriptorId::from_raw_fd(fd)) {
            Some(fd_info) if fd_info.is_passthrough => {
                let dupfd = hook_macros::real!(fcntl)(fd, cmd, arg);
                if dupfd >= 0 && (cmd == libc::F_DUPFD || cmd == libc::F_DUPFD_CLOEXEC) {
                    let nonblocking = fd_info.nonblocking;
                    let close_on_exec = cmd == libc::F_DUPFD_CLOEXEC;
                    let resource = fd_info.resource.clone();
                    state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(dupfd), DescriptorInfo {
                        close_on_exec,
                        nonblocking,
                        is_passthrough: true,
                        resource,
                    }).unwrap();
                }

                dupfd
            }
            Some(fd_info) => {
                match cmd {
                    libc::F_GETFL => return if fd_info.nonblocking { libc::O_NONBLOCK } else { 0 }, // TODO: support remaining flags
                    libc::F_SETFL => {
                        fd_info.nonblocking = ((arg as usize) & libc::O_NONBLOCK as usize) > 0;
                        return 0
                    }
                    libc::F_GETFD => return if fd_info.close_on_exec { libc::O_CLOEXEC } else { 0 },
                    libc::F_SETFD => {
                        fd_info.close_on_exec = ((arg as usize) & libc::O_CLOEXEC as usize) > 0;
                        return 0;
                    }
                    libc::F_DUPFD | libc::F_DUPFD_CLOEXEC => {
                        let nonblocking = fd_info.nonblocking;
                        let resource = fd_info.resource.clone();

                        let dupfd = crate::create_descriptor();
                        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(dupfd), DescriptorInfo {
                            close_on_exec: cmd == libc::F_DUPFD_CLOEXEC,
                            nonblocking,
                            is_passthrough: false,
                            resource,
                        }).unwrap();

                        return dupfd
                    }
                    libc::F_SETLK | libc::F_SETLKW | libc::F_GETLK => {
                        crate::report_strict_failure("unimplemented fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_GETOWN | libc::F_SETOWN => {
                        crate::report_strict_failure("unimplemented OWN fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_GETLEASE | libc::F_SETLEASE => {
                        crate::report_strict_failure("unimplemented LEASE fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_NOTIFY => {
                        crate::report_strict_failure("unimplemented fcntl command F_NOTIFY");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_SETPIPE_SZ => {
                        crate::report_strict_failure("unimplemented fcntl command F_SETPIPE_SZ");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_ADD_SEALS | libc::F_GET_SEALS => {
                        crate::report_strict_failure("unimplemented fcntl command(s) SEALS");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    // libc::F_GET_RW_HINT | libc::F_SET_RW_HINT | libc::F_GET_FILE_RW_HINT | libc::F_SET_FILE_RW_HINT => unimplemented by libc
                    _ => {
                        crate::report_strict_failure("unrecognized fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    },
                }
            },
            None => {
                *libc::__errno_location() = libc::EBADF;
                return -1
            },
        }


    }
}

// GNU libc unconditionally pulls a void* from va_args, so we should (hypothetically?) be okay doing this.
hook_macros::hook! {
    unsafe fn ioctl(
        fd: libc::c_int,
        request: libc::c_int,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_ioctl(_ctx) {
        log::info!("ioctl({}, {}, {})", fd, request, arg as usize);

        panic!("`ioctl` unimplemented")
    }
}
