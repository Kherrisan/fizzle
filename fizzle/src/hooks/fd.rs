//! Hooks for general functions that can be applied to any file descriptor.
//! 

use crate::{hook_macros, state::{self, fd::FdInfo}};

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close(ctx) {

        match ctx.local().fds.remove(&fd) {
            Some(FdInfo::Directory(_)) => crate::alias_fd_destroy(fd),
            Some(FdInfo::File(_)) => crate::alias_fd_destroy(fd),
            Some(FdInfo::PassthroughFile(_)) => return hook_macros::real!(close)(fd),
            None => {
                *libc::__errno_location() = libc::EBADFD;
                return -1
            },
        }

        0
    }
}



