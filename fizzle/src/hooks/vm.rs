use crate::hook_macros;

hook_macros::hook! {
    unsafe fn vmsplice(
        fd: libc::c_int,
        iov: *const libc::iovec,
        nr_segs: libc::size_t,
        flags: libc::c_uint
    ) -> libc::ssize_t => fizzle_vmsplice(_ctx) {
        unimplemented!("vmsplice()")
    }
}

