use crate::hook_macros;

hook_macros::hook! {
    unsafe fn openlog(
        ident: *const libc::c_char,
        option: libc::c_int,
        facility: libc::c_int
    ) => fizzle_openlog(_ctx) {

        crate::strace!("openlog(ident={:?}, option={}, facility={}) -> ()", ident, option, facility);
    }
}

hook_macros::hook! {
    unsafe fn syslog(
        priority: libc::c_int
    ) => fizzle_syslog(_ctx) {

        crate::strace!("syslog(priority={}, ...) -> ()", priority);
    }
}

hook_macros::hook! {
    unsafe fn vsyslog(
        priority: libc::c_int,
        format: *const libc::c_char
    ) => fizzle_vsyslog(_ctx) {
        crate::strace!("vsyslog(priority={}, format={:?}, ...) -> ()", priority, format);
    }
}

hook_macros::hook! {
    unsafe fn closelog() => fizzle_closelog(_ctx) {
        crate::strace!("closelog() -> ()");
    }
}

