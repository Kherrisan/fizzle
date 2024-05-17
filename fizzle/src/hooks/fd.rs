//! Hooks for general functions that can be applied to any file descriptor.
//! 

use crate::{hook_macros, state::{self, fd::FdInfo}};

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close {

        let mut state = state::fizzle_state().lock().unwrap();
        match state.fds.remove(&fd) {
            Some(FdInfo::Directory(_)) => 0,
            Some(FdInfo::File(_)) => 0,
            Some(FdInfo::PassthroughFile(_)) => hook_macros::real!(close)(fd),
            None => {
                *libc::__errno_location() = libc::EBADFD;
                -1
            },
        }
    }
}



