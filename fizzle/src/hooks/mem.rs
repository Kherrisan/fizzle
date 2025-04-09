use std::ffi::CStr;

use crate::hook_macros;

hook_macros::hook! {
    unsafe fn memfd_create(
        _name: *const libc::c_char,
        _flags: libc::c_uint
    ) => fizzle_memfd_create(_ctx) {
        unimplemented!("memfd_create()")
    }
}

// mmap, munmap




hook_macros::hook! {
    unsafe fn strdup(
        name: *const libc::c_char
    ) -> *mut libc::c_char => fizzle_strdup(_ctx) {
        let name_cstr = CStr::from_ptr(name);

        log::debug!("strdup({:?}", name_cstr);

        libc::strdup(name)
    }
}
