use crate::hook_macros;

hook_macros::hook! {
    unsafe fn opendir(
        name: *const libc::c_char
    ) -> *mut libc::DIR => fizzle_opendir(_ctx) {
        unimplemented!("opendir()")
    }
}

hook_macros::hook! {
    unsafe fn fdopendir(
        fd: libc::c_int
    ) -> *mut libc::DIR => fizzle_fdopendir(_ctx) {
        unimplemented!("fdopendir()")
    }
}

hook_macros::hook! {
    unsafe fn dirfd(
        _dirp: *mut libc::DIR
    ) => fizzle_dirfd(_ctx) {
        unimplemented!("dirfd()")
    }
}
