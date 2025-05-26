use crate::{constants::FIZZLE_UID_ENV, hook_macros};
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn getuid() -> libc::uid_t => fizzle_getuid(ctx) {
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
    unsafe fn setuid(uid: libc::uid_t) -> libc::c_int => fizzle_setuid(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function setuid() called within signal handler")
            }
        }

        crate::strace!("setuid({}) -> ...", uid);

        log::warn!("unimplemented: setuid({})", uid);

        crate::strace!("setuid({}) -> 0", uid);
        0
    }
}

hook_macros::hook! {
    unsafe fn geteuid() -> libc::uid_t => fizzle_geteuid(ctx) {
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
    unsafe fn seteuid(uid: libc::uid_t) -> libc::c_int => fizzle_seteuid(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function seteuid() called within signal handler")
            }
        }

        crate::strace!("seteuid({}) -> ...", uid);

        log::warn!("unimplemented: seteuid({})", uid);

        crate::strace!("seteuid({}) -> 0", uid);
        0
    }
}

hook_macros::hook! {
    unsafe fn getgid() -> libc::uid_t => fizzle_getgid(ctx) {
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
    unsafe fn setgid(uid: libc::uid_t) -> libc::c_int => fizzle_setgid(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function setgid() called within signal handler")
            }
        }

        crate::strace!("setgid({}) -> ...", uid);

        log::warn!("unimplemented: setgid({})", uid);

        crate::strace!("setgid({}) -> 0", uid);
        0
    }
}

hook_macros::hook! {
    unsafe fn getegid() -> libc::uid_t => fizzle_getegid(ctx) {
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
    unsafe fn setegid(uid: libc::uid_t) -> libc::c_int => fizzle_setegid(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function setegid() called within signal handler")
            }
        }

        crate::strace!("setegid({}) -> ...", uid);

        log::warn!("unimplemented: setegid({})", uid);

        crate::strace!("setegid({}) -> 0", uid);
        0
    }
}

// TODO: hook `getprotoent`, `getpwnam`, `getgrnam`

hook_macros::hook! {
    unsafe fn getpwnam(name: *const libc::c_char) -> *mut libc::passwd => fizzle_getpwnam(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function getpwnam() called within signal handler")
            }
        }

        crate::strace!("getpwnam(name={:?}) -> ...", name);

        log::warn!("unimplemented: getpwnam() (passthrough)");

        let ret = libc::getpwnam(name);

        crate::strace!("getpwnam(name={:?}) -> {:?}", name, ret);
        
        ret
    }
}

hook_macros::hook! {
    unsafe fn getpwuid(uid: libc::uid_t) -> *mut libc::passwd => fizzle_getpwuid(ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function getpwuid() called within signal handler")
            }
        }

        crate::strace!("getpwuid(uid={}) -> ...", uid);

        log::warn!("unimplemented: getpwuid() (passthrough)");

        let ret = libc::getpwuid(uid);

        crate::strace!("getpwuid(uid={}) -> {:?}", uid, ret);
        
        ret
    }
}
