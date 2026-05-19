use std::ffi::CStr;

use crate::{constants::FIZZLE_UID_ENV, hook_macros};

hook_macros::hook! {
    unsafe fn getuid() -> libc::uid_t => fizzle_getuid(_ctx) {
        crate::strace!("getuid() -> ...");

        if let Ok(uid) = std::env::var(FIZZLE_UID_ENV) {
            let uid: libc::uid_t = uid.parse().unwrap();
            crate::strace!("getuid() -> {}", uid);
            return uid
        }

        crate::strace!("getuid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn setuid(uid: libc::uid_t) -> libc::c_int => fizzle_setuid(_ctx) {
        crate::strace!("setuid({}) -> ...", uid);

        log::warn!("unimplemented: setuid({})", uid);

        crate::strace!("setuid({}) -> 0", uid);
        0
    }
}

hook_macros::hook! {
    unsafe fn geteuid() -> libc::uid_t => fizzle_geteuid(_ctx) {
        crate::strace!("geteuid() -> ...");

        if let Ok(uid) = std::env::var(FIZZLE_UID_ENV) {
            let uid: libc::uid_t = uid.parse().unwrap();
            crate::strace!("geteuid() -> {}", uid);
            return uid
        }

        crate::strace!("geteuid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn seteuid(uid: libc::uid_t) -> libc::c_int => fizzle_seteuid(_ctx) {
        crate::strace!("seteuid({}) -> ...", uid);

        log::warn!("unimplemented: seteuid({})", uid);

        crate::strace!("seteuid({}) -> 0", uid);
        0
    }
}

hook_macros::hook! {
    unsafe fn getgid() -> libc::uid_t => fizzle_getgid(_ctx) {
        crate::strace!("getgid() -> ...");

        if let Ok(uid) = std::env::var(FIZZLE_UID_ENV) {
            let uid: libc::uid_t = uid.parse().unwrap();
            crate::strace!("getgid() -> {}", uid);
            return uid
        }

        crate::strace!("getgid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn setgid(uid: libc::uid_t) -> libc::c_int => fizzle_setgid(_ctx) {
        crate::strace!("setgid({}) -> ...", uid);

        log::warn!("unimplemented: setgid({})", uid);

        crate::strace!("setgid({}) -> 0", uid);
        0
    }
}

hook_macros::hook! {
    unsafe fn getegid() -> libc::uid_t => fizzle_getegid(_ctx) {
        crate::strace!("getegid() -> ...");

        if let Ok(uid) = std::env::var(FIZZLE_UID_ENV) {
            let uid: libc::uid_t = uid.parse().unwrap();
            crate::strace!("getegid() -> {}", uid);
            return uid
        }

        crate::strace!("getegid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn setegid(uid: libc::uid_t) -> libc::c_int => fizzle_setegid(_ctx) {
        crate::strace!("setegid({}) -> ...", uid);

        log::warn!("unimplemented: setegid({})", uid);

        crate::strace!("setegid({}) -> 0", uid);
        0
    }
}

// TODO: hook `getprotoent`, `getpwnam`, `getgrnam`

hook_macros::hook! {
    unsafe fn getpwnam(name: *const libc::c_char) -> *mut libc::passwd => fizzle_getpwnam(_ctx) {
        let name_cstr = CStr::from_ptr(name);

        crate::strace!("getpwnam(name={name_cstr:?}) -> ...");

        log::warn!("unimplemented: getpwnam() (passthrough)");

        let ret = libc::getpwnam(name);

        crate::strace!("getpwnam(name={name_cstr:?}) -> {:?}", ret);
        
        ret
    }
}

hook_macros::hook! {
    unsafe fn getpwuid(uid: libc::uid_t) -> *mut libc::passwd => fizzle_getpwuid(_ctx) {
        crate::strace!("getpwuid(uid={}) -> ...", uid);

        log::warn!("unimplemented: getpwuid() (passthrough)");

        let ret = libc::getpwuid(uid);

        crate::strace!("getpwuid(uid={}) -> {:?}", uid, ret);
        
        ret
    }
}
