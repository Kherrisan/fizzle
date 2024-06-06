use core::slice;
use std::mem::MaybeUninit;

use crate::hook_macros;

hook_macros::hook! {
    unsafe fn getrandom(
        buf: *mut libc::c_void,
        buflen: libc::size_t,
        _flags: libc::c_uint
    ) -> libc::ssize_t => fizzle_getrandom(ctx) {

        let output = slice::from_raw_parts_mut(buf as *mut MaybeUninit<u8>, buflen);
        ctx.global().gen_random_bytes(output);

        buflen as libc::ssize_t
    }
}



hook_macros::hook! {
    unsafe fn srand(
        _seed: libc::c_uint
    ) => fizzle_srand(_ctx) {
        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn rand() -> libc::c_int => fizzle_rand(ctx) {
        libc::c_int::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn arc4random() -> u32 => fizzle_arc4random(ctx) {
        u32::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn arc4random_uniform(upper_bound: u32) -> u32 => fizzle_arc4random_uniform(ctx) {
        match upper_bound {
            0 => 0,
            _ => u32::from_ne_bytes(ctx.global().gen_random_array()) % upper_bound,
        }
    }
}

hook_macros::hook! {
    unsafe fn arc4random_buf(buf: *mut libc::c_void, n: libc::size_t) => fizzle_arc4random_buf(ctx) {
        let output = slice::from_raw_parts_mut(buf as *mut MaybeUninit<u8>, n);
        ctx.global().gen_random_bytes(output);
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
    unsafe fn random() -> libc::c_uint => fizzle_random(ctx) {
        libc::c_uint::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn srandom(_seed: libc::c_uint) => fizzle_srandom(_ctx) {
        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn drand48() -> libc::c_double => fizzle_drand48(ctx) {
        libc::c_double::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn erand48(_xsubi: *mut libc::c_ushort) -> libc::c_double => fizzle_erand48(ctx) {
        libc::c_double::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn lrand48() -> libc::c_long => fizzle_lrand48(ctx) {
        libc::c_long::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn nrand48(_xsubi: *mut libc::c_ushort) -> libc::c_long => fizzle_nrand48(ctx) {
        libc::c_long::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn mrand48() -> libc::c_long => fizzle_mrand48(ctx) {
        libc::c_long::from_ne_bytes(ctx.global().gen_random_array())
    }
}

hook_macros::hook! {
    unsafe fn jrand48(_xsubi: *mut libc::c_ushort) -> libc::c_long => fizzle_jrand48(ctx) {
        libc::c_long::from_ne_bytes(ctx.global().gen_random_array())
    }
}
