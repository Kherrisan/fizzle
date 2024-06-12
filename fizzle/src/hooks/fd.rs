//! Hooks for general functions that can be applied to any file descriptor.
//!

use crate::hook_macros;

use crate::state::fd::{FdInfo, FdResource};
use crate::state::identifiers::DescriptorId;

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close(ctx) {
        let descriptor_id = DescriptorId::new(fd);

        // TODO: remove underlying resource from descriptor for each of these options (otherwise memory leak + state error)
        match ctx.local.fds.remove(descriptor_id) {
            Some(FdInfo { resource: FdResource::Epoll(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::Directory(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::File(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::Socket(_), .. }) => return hook_macros::real!(close)(fd),
            Some(FdInfo { resource: FdResource::MessageQueue(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::Pipe(_), .. }) => crate::alias_fd_destroy(fd),
            // TODO: mark stdin as closed after this...
            Some(FdInfo { resource: FdResource::Stdin, .. }) => (), // We keep stdin for fuzzing input...
            Some(FdInfo { resource: FdResource::Stdout, .. }) => (),
            Some(FdInfo { resource: FdResource::Stderr, .. }) => (), // ... and stderr for reporting Fizzle errors.
            None => {
                *libc::__errno_location() = libc::EBADFD;
                return -1
            },
        }

        // TODO: implement cleanup properly here

        0
    }
}

// GNU libc unconditionally pulls a void* from va_args, so we should (hypothetically?) be okay doing this.
hook_macros::hook! {
    unsafe fn fcntl(
        fd: libc::c_int,
        cmd: libc::c_int,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_fcntl(ctx) {

        match ctx.local.fds.get_mut(DescriptorId::new(fd)) {
            Some(fd_info) if fd_info.is_passthrough => {
                let dupfd = hook_macros::real!(fcntl)(fd, cmd, arg);
                if dupfd >= 0 && (cmd == libc::F_DUPFD || cmd == libc::F_DUPFD_CLOEXEC) {
                    let nonblocking = fd_info.nonblocking;
                    let close_on_exec = cmd == libc::F_DUPFD_CLOEXEC;
                    let resource = fd_info.resource;
                    ctx.local.fds.insert(DescriptorId::new(dupfd), FdInfo {
                        close_on_exec,
                        nonblocking,
                        is_passthrough: true,
                        resource,
                    });
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
                        let resource = fd_info.resource;

                        let dupfd = crate::alias_fd_create();
                        ctx.local.fds.insert(DescriptorId::new(dupfd), FdInfo {
                            close_on_exec: cmd == libc::F_DUPFD_CLOEXEC,
                            nonblocking,
                            is_passthrough: false,
                            resource,
                        });
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
        crate::report_strict_failure("`ioctl` unimplemented");
        hook_macros::real!(ioctl)(fd, request, arg)
    }
}
