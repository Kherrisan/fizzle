//! Process I/O shims.
//! 
//! 

use crate::{hook_macros, state};

hook_macros::hook! {
    unsafe fn socket(
        domain: libc::c_int,
        socktype: libc::c_int,
        protocol: libc::c_int
    ) -> libc::c_int => fizzle_socket(_ctx) {


        hook_macros::real!(socket)(domain, socktype, protocol)

    }
}

