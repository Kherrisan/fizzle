use std::{env, ffi::CStr};

use crate::constants::FIZZLE_SINGLEPROCESS_ENV;
use crate::hook_macros;
use crate::scheduler;
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn memfd_create(
        _name: *const libc::c_char,
        _flags: libc::c_uint
    ) => fizzle_memfd_create(_ctx) {
        unimplemented!("memfd_create()")
    }
}

// mmap, munmap

// TODO: in the future, just call `afl_onetime_init` here if not in FIZZLE_SINGLEPROCESS mode
hook_macros::hook! {
    unsafe fn mmap(
        addr: *mut libc::c_void,
        length: libc::size_t,
        prot: libc::c_int,
        flags: libc::c_int,
        fd: libc::c_int,
        offset: libc::off_t
    ) -> *mut libc::c_void => fizzle_mmap(ctx) {

        let mut flags = flags;

        crate::strace!("mmap(addr={:?}, length={}, prot={}, flags={}, fd={}, offset={}) -> ...", addr, length, prot, flags, fd, offset);

        let is_singleprocess = matches!(env::var(FIZZLE_SINGLEPROCESS_ENV), Ok(s) if s == "1");

        if flags & (libc::MAP_SHARED | libc::MAP_SHARED_VALIDATE) > 0 {
            if is_singleprocess {
                log::warn!("disabling MAP_SHARED for mmap()");
                flags &= !(libc::MAP_SHARED | libc::MAP_SHARED_VALIDATE);
            } else {
                scheduler::afl_onetime_init(&mut ctx);
            }
        }

        if fd >= 0 {
            if is_singleprocess {
                log::warn!("adding MAP_PRIVATE for mmap() with underlying fd");
                flags |= libc::MAP_PRIVATE; 
            } else {
                scheduler::afl_onetime_init(&mut ctx);
            }
        }

        libc::mmap(addr, length, prot, flags, fd, offset)
    }
}


hook_macros::hook! {
    unsafe fn strdup(
        name: *const libc::c_char
    ) -> *mut libc::c_char => fizzle_strdup(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function strdup() called within signal handler")
            }
        }

        let name_cstr = CStr::from_ptr(name);

        log::debug!("strdup({:?})", name_cstr);

        libc::strdup(name)
    }
}
