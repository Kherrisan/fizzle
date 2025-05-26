use crate::hook_macros;
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn openlog(
        ident: *const libc::c_char,
        option: libc::c_int,
        facility: libc::c_int
    ) => fizzle_openlog(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function openlog() called within signal handler")
            }
        }

        crate::strace!("openlog(ident={:?}, option={}, facility={}) -> ()", ident, option, facility);
    }
}

hook_macros::hook! {
    unsafe fn syslog(
        priority: libc::c_int
    ) => fizzle_syslog(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function syslog() called within signal handler")
            }
        }

        crate::strace!("syslog(priority={}, ...) -> ()", priority);
    }
}

hook_macros::hook! {
    unsafe fn vsyslog(
        priority: libc::c_int,
        format: *const libc::c_char
    ) => fizzle_vsyslog(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function vsyslog() called within signal handler")
            }
        }

        crate::strace!("vsyslog(priority={}, format={:?}, ...) -> ()", priority, format);
    }
}

hook_macros::hook! {
    unsafe fn closelog() => fizzle_closelog(_ctx) {
        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function closelog() called within signal handler")
            }
        }
        crate::strace!("closelog() -> ()");
    }
}

