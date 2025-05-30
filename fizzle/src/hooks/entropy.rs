use std::{mem, slice};

use crate::handlers::entropy::GetEntropyEvent;
use crate::hook_macros;
use crate::scheduler::Scheduler;
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn getrandom(
        buf: *mut libc::c_void,
        buflen: libc::size_t,
        flags: libc::c_uint
    ) -> libc::ssize_t => fizzle_getrandom(ctx) {
        crate::strace!("getrandom(buf={:?}, buflen={}, flags={}) -> ...", buf, buflen, flags);
        let s = slice::from_raw_parts_mut(buf.cast::<u8>(), buflen);
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(s)) {
            Ok(len) => {
                crate::strace!("getrandom(buf={:?}, buflen={}, flags={}) -> {:.16?}", buf, buflen, flags, &s[..len]);
                len as isize
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn getentropy(
        buf: *mut libc::c_void,
        buflen: libc::size_t
    ) -> libc::ssize_t => fizzle_getentropy(ctx) {
        crate::strace!("getentropy(buf={:?}, buflen={}) -> ...", buf, buflen);
        let s = slice::from_raw_parts_mut(buf.cast::<u8>(), buflen);
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(s)) {
            Ok(len) => {
                crate::strace!("getentropy(buf={:?}, buflen={}) -> {:.16?}", buf, buflen, &s[..len]);
                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn srand(
        _seed: libc::c_uint
    ) => fizzle_srand(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function srand() called within signal handler")
            }
        }

        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn rand() -> libc::c_int => fizzle_rand(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function rand() called within signal handler")
            }
        }

        const INT_SIZE: usize = mem::size_of::<libc::c_int>();
        let mut int_array = [0u8; INT_SIZE];

        crate::strace!("rand() -> ...");
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(int_array.as_mut_slice())) {
            Ok(INT_SIZE) => {
                let out = libc::c_int::from_ne_bytes(int_array);
                crate::strace!("rand() -> {}", out);
                out
            },
            _ => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn srandom(
        _seed: libc::c_uint
    ) => fizzle_srandom(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function srandom() called within signal handler")
            }
        }

        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn random() -> libc::c_long => fizzle_random(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function random() called within signal handler")
            }
        }

        const INT_SIZE: usize = mem::size_of::<libc::c_long>();
        let mut int_array = [0u8; INT_SIZE];

        crate::strace!("random() -> ...");
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(int_array.as_mut_slice())) {
            Ok(INT_SIZE) => {
                let out = libc::c_long::from_ne_bytes(int_array);
                crate::strace!("random() -> {}", out);
                out
            },
            _ => unreachable!(),
        }
    }
}


hook_macros::hook! {
    unsafe fn arc4random() -> u32 => fizzle_arc4random(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function arc4random() called within signal handler")
            }
        }

        const U32_SIZE: usize = mem::size_of::<u32>();
        let mut int_array = [0u8; U32_SIZE];

        crate::strace!("arc4random() -> ...");
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(int_array.as_mut_slice())) {
            Ok(U32_SIZE) => {
                let out = u32::from_ne_bytes(int_array);
                crate::strace!("arc4random() -> {}", out);
                out
            },
            _ => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn arc4random_uniform(upper_bound: u32) -> u32 => fizzle_arc4random_uniform(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function arc4random_uniform() called within signal handler")
            }
        }

        const U32_SIZE: usize = mem::size_of::<u32>();
        let mut int_array = [0u8; U32_SIZE];

        crate::strace!("arc4random_uniform(ub={}) -> ...", upper_bound);
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(int_array.as_mut_slice())) {
            Ok(U32_SIZE) => {
                // TODO: this is not quite uniformly distributed...
                let out = u32::from_ne_bytes(int_array) % upper_bound;
                crate::strace!("arc4random_uniform(ub={}) -> {}", upper_bound, out);
                out
            },
            _ => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn arc4random_buf(buf: *mut libc::c_void, n: libc::size_t) => fizzle_arc4random_buf(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function arc4random_buf() called within signal handler")
            }
        }

        crate::strace!("arc4random_buf(buf={:?}, n={}) -> ...", buf, n);
        let s = slice::from_raw_parts_mut(buf.cast::<u8>(), n);
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(s)) {
            Ok(len) if len < n => unreachable!(),
            Ok(_) => {
                crate::strace!("arc4random_buf(buf={:.16?}, n={})", s, n);
            },
            Err(_) => unreachable!(),
        }
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
    unsafe fn srand48(_seedval: libc::c_long) => fizzle_srand48(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function srand48() called within signal handler")
            }
        }

        // Do nothing
    }
}

hook_macros::hook! {
    unsafe fn seed48(_seed16v: *mut libc::c_ushort) -> *mut libc::c_ushort => fizzle_seed48(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function seed48() called within signal handler")
            }
        }

        unimplemented!("seed48()")
    }
}

hook_macros::hook! {
    unsafe fn lcong48(_param: *mut libc::c_ushort) => fizzle_lcong48(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function lcong48() called within signal handler")
            }
        }

        unimplemented!("lcong48()")
    }
}

hook_macros::hook! {
    unsafe fn drand48() -> libc::c_double => fizzle_drand48(_ctx) {
        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function drand48() called within signal handler")
            }
        }

        // Needs to return uniform sample from 0.0 to 1.0 double precision
        unimplemented!("drand48")
    }
}

hook_macros::hook! {
    unsafe fn erand48(_xsubi: *mut libc::c_ushort) -> libc::c_double => fizzle_erand48(_ctx) {
        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function erand48() called within signal handler")
            }
        }
        unimplemented!("erand48")
    }
}

hook_macros::hook! {
    unsafe fn lrand48() -> libc::c_long => fizzle_lrand48(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function lrand48() called within signal handler")
            }
        }


        const LONG_SIZE: usize = mem::size_of::<libc::c_long>();
        let mut long_array = [0u8; LONG_SIZE];

        crate::strace!("lrand48() -> ...");
        match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(long_array.as_mut_slice())) {
            Ok(LONG_SIZE) => {
                let mut out = libc::c_long::from_ne_bytes(long_array);
                out %= (1 << 31) + 1;
                crate::strace!("lrand48() -> {}", out);
                out
            },
            _ => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn nrand48(_xsubi: *mut libc::c_ushort) -> libc::c_long => fizzle_nrand48(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function nrand48() called within signal handler")
            }
        }

        unimplemented!("nrand48")
    }
}

hook_macros::hook! {
    unsafe fn mrand48() -> libc::c_long => fizzle_mrand48(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function mrand48() called within signal handler")
            }
        }

        unimplemented!("mrand48")
    }
}

hook_macros::hook! {
    unsafe fn jrand48(_xsubi: *mut libc::c_ushort) -> libc::c_long => fizzle_jrand48(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function jrand48() called within signal handler")
            }
        }

        unimplemented!("jrand48")
    }
}
