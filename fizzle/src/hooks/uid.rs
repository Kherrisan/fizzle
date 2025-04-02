use crate::hook_macros;

hook_macros::hook! {
    unsafe fn getuid() -> libc::uid_t => fizzle_getuid(ctx) {
        crate::strace!("getuid() -> ...");

        crate::strace!("getuid() -> 1001");
        1001
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
    unsafe fn getgid() -> libc::uid_t => fizzle_getgid(ctx) {
        crate::strace!("getgid() -> ...");

        crate::strace!("getgid() -> 1001");
        1001
    }
}

hook_macros::hook! {
    unsafe fn getegid() -> libc::uid_t => fizzle_getegid(ctx) {
        crate::strace!("getegid() -> ...");

        crate::strace!("getegid() -> 1001");
        1001
    }
}
