//! Hooks for general functions that can be applied to any file descriptor.
//!

use crate::hook_macros;
use crate::state::fd::{FdInfo, FdResource};
use crate::state::DescriptorId;

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close(ctx) {
        let descriptor_id = DescriptorId::new(fd);

        match ctx.local().fds.remove(descriptor_id) {
            Some(FdInfo { resource: FdResource::Directory(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::File(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::Socket(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::MessageQueue(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::Pipe(_), .. }) => crate::alias_fd_destroy(fd),
            Some(FdInfo { resource: FdResource::PassthroughFile, .. }) => return hook_macros::real!(close)(fd),
            None => {
                *libc::__errno_location() = libc::EBADFD;
                return -1
            },
        }

        0
    }
}
