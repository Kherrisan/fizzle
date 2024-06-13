use crate::hook_macros;

hook_macros::hook! {
    unsafe fn eventfd(
        initval: libc::c_uint,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_eventfd(_ctx) {
        hook_macros::real!(eventfd)(initval, flags)
    }
}
