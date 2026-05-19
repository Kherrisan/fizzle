use std::{env, ffi::CStr};

use crate::constants::FIZZLE_SINGLEPROCESS_ENV;
use crate::errno::Errno;
use crate::hook_macros;
use crate::scheduler;

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

        crate::strace!("mmap(addr={addr:?}, length={length}, prot={prot}, flags={flags}, fd={fd}, offset={offset}) -> ...");

        let is_singleprocess = matches!(env::var(FIZZLE_SINGLEPROCESS_ENV), Ok(s) if s == "1");

        if flags & libc::MAP_SHARED > 0 {
            if is_singleprocess {
                log::warn!("disabling MAP_SHARED for mmap()");
                flags &= !libc::MAP_SHARED;
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

        let res = libc::mmap(addr, length, prot, flags, fd, offset);

        if res == libc::MAP_FAILED {
            crate::strace!("mmap(addr={addr:?}, length={length}, prot={prot}, flags={flags}, fd={fd}, offset={offset}) -> -1 ({})", Errno::get_errno());
        } else {
            crate::strace!("mmap(addr={addr:?}, length={length}, prot={prot}, flags={flags}, fd={fd}, offset={offset}) -> {res:?}");
        }

        res
    }
}


hook_macros::hook! {
    unsafe fn strdup(
        name: *const libc::c_char
    ) -> *mut libc::c_char => fizzle_strdup(_ctx) {

        let name_cstr = CStr::from_ptr(name);

        log::debug!("strdup({:?})", name_cstr);

        libc::strdup(name)
    }
}
