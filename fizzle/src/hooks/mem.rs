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
