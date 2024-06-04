use crate::hook_macros;

hook_macros::hook! {
    unsafe fn getrandom(
        _buf: *mut libc::c_void,
        _buflen: libc::size_t,
        _flags: libc::c_uint
    ) -> libc::ssize_t => fizzle_getrandom(_ctx) {

        panic!("`getrandom` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn srand(
        _seed: libc::c_uint
    ) => fizzle_srand(_ctx) {

        panic!("`srand` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn rand() -> libc::c_int => fizzle_rand(_ctx) {

        panic!("`rand` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn arc4random() -> u32 => fizzle_arc4random(_ctx) {

        panic!("`arc4random` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn arc4random_uniform(_upper_bound: u32) -> u32 => fizzle_arc4random_uniform(_ctx) {

        panic!("`arc4random_uniform` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn arc4random_buf(_buf: *mut libc::c_void, _n: libc::size_t) => fizzle_arc4random_buf(_ctx) {

        panic!("`arc4random_buf` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn arc4random_stir() => fizzle_arc4random_stir(_ctx) {
        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn arc4random_addrandom(_dat: *mut libc::c_uchar, _datlen: libc::c_int) => fizzle_arc4random_addrandom(_ctx) {
        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn random() -> libc::c_uint => fizzle_random(_ctx) {
        panic!("`random` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn srandom(_seed: libc::c_uint) => fizzle_srandom(_ctx) {
        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn drand48() -> libc::c_double => fizzle_drand48(_ctx) {
        panic!("`drand48` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn erand48(_xsubi: *mut libc::c_ushort) -> libc::c_double => fizzle_erand48(_ctx) {
        panic!("`erand48` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn lrand48() -> libc::c_long => fizzle_lrand48(_ctx) {
        panic!("`lrand48` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn nrand48(_xsubi: *mut libc::c_ushort) -> libc::c_long => fizzle_nrand48(_ctx) {
        panic!("`nrand48` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn mrand48() -> libc::c_long => fizzle_mrand48(_ctx) {
        panic!("`mrand48` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn jrand48(_xsubi: *mut libc::c_ushort) -> libc::c_long => fizzle_jrand48(_ctx) {
        panic!("`jrand48` unimplemented")
    }
}
