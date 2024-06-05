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
        match ctx.local().fds.remove(descriptor_id) {
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
        hook_macros::real!(fcntl)(fd, cmd, arg)
    }
}
