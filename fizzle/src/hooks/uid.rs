use crate::hook_macros;

hook_macros::hook! {
    unsafe fn getuid() -> libc::uid_t => fizzle_getuid(ctx) {
        crate::strace!("getuid() -> ...");

        crate::strace!("getuid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn setuid(uid: libc::uid_t) -> libc::c_int => fizzle_setuid(ctx) {
        crate::strace!("setuid() -> ...");

        log::warn!("unimplemented: setuid()");

        crate::strace!("setuid() -> 0");
        0
    }
}

hook_macros::hook! {
    unsafe fn geteuid() -> libc::uid_t => fizzle_geteuid(ctx) {
        crate::strace!("geteuid() -> ...");

        crate::strace!("geteuid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn seteuid(uid: libc::uid_t) -> libc::c_int => fizzle_seteuid(ctx) {
        crate::strace!("seteuid() -> ...");

        log::warn!("unimplemented: seteuid()");

        crate::strace!("seteuid() -> 0");
        0
    }
}

hook_macros::hook! {
    unsafe fn getgid() -> libc::uid_t => fizzle_getgid(ctx) {
        crate::strace!("getgid() -> ...");

        crate::strace!("getgid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn setgid(uid: libc::uid_t) -> libc::c_int => fizzle_setgid(ctx) {
        crate::strace!("setgid() -> ...");

        log::warn!("unimplemented: setgid()");

        crate::strace!("setgid() -> 0");
        0
    }
}

hook_macros::hook! {
    unsafe fn getegid() -> libc::uid_t => fizzle_getegid(ctx) {
        crate::strace!("getegid() -> ...");

        crate::strace!("getegid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn setegid(uid: libc::uid_t) -> libc::c_int => fizzle_setegid(ctx) {
        crate::strace!("setegid() -> ...");

        log::warn!("unimplemented: setegid()");

        crate::strace!("setegid() -> 0");
        0
    }
}

hook_macros::hook! {
    unsafe fn getpwnam(name: *const libc::c_char) -> *mut libc::passwd => fizzle_getpwnam(ctx) {
        crate::strace!("getpwnam(name={:?}) -> ...", name);

        log::warn!("unimplemented: getpwnam() (passthrough)");

        let ret = libc::getpwnam(name);

        crate::strace!("getpwnam(name={:?}) -> {:?}", name, ret);
        
        ret
    }
}

hook_macros::hook! {
    unsafe fn getpwuid(uid: libc::uid_t) -> *mut libc::passwd => fizzle_getpwuid(ctx) {
        crate::strace!("getpwuid(uid={}) -> ...", uid);

        log::warn!("unimplemented: getpwuid() (passthrough)");

        let ret = libc::getpwuid(uid);

        crate::strace!("getpwuid(uid={}) -> {:?}", uid, ret);
        
        ret
    }
}
