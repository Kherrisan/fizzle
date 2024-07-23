use crate::hook_macros;

hook_macros::hook! {
    unsafe fn sigwait(
        _set: *const libc::sigset_t,
        _sig: *mut libc::c_int
    ) -> libc::c_int => fizzle_sigwait(ctx) {
        // TODO: handle signals in the future
        ctx.yield_thread();

        libc::EINVAL
    }
}

hook_macros::hook! {
    unsafe fn sigwaitinfo(
        _set: *const libc::sigset_t,
        _info: *mut libc::siginfo_t
    ) -> libc::c_int => fizzle_sigwaitinfo(ctx) {
        ctx.yield_thread();

        libc::EINVAL
    }
}

hook_macros::hook! {
    unsafe fn sigtimedwait(
        _set: *const libc::sigset_t,
        _info: *mut libc::siginfo_t,
        _timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_sigtimedwait(ctx) {
        ctx.yield_thread();

        libc::EINVAL
    }
}

hook_macros::hook! {
    unsafe fn signal(
        _signum: libc::c_int,
        _handler: libc::sighandler_t
    ) -> libc::sighandler_t => fizzle_signal(_ctx) {
        log::error!("signal() unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn kill(
        _pid: libc::pid_t,
        _sig: libc::c_int
    ) -> libc::c_int => fizzle_kill(_ctx) {
        panic!("kill() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn signalfd(
        _fd: libc::c_int,
        _mask: *const libc::sigset_t,
        _flags: libc::c_int
    ) -> libc::c_int => fizzle_signalfd(_ctx) {
        log::error!("signalfd() unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn sigaction(
        _act: *const libc::sigaction,
        _oldact: *mut libc::sigaction
    ) -> libc::c_int => fizzle_sigaction(_ctx) {
        log::error!("sigaction() unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn setitimer(
        _which: libc::c_int,
        _new_value: *mut libc::itimerval,
        _old_value: *mut libc::itimerval
    ) -> libc::c_int => fizzle_setitimer(_ctx) {
        log::error!("setitimer() unimplemented");
        0
    }
}

hook_macros::hook!{
    unsafe fn getitimer(
        _which: libc::c_int,
        _curr_value: *mut libc::itimerval
    ) -> libc::c_int => fizzle_getitimer(_ctx) {
        log::error!("getitimer() unimplemented");
        0
    }
}

hook_macros::hook!{
    unsafe fn sigsuspend(
        _mask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_sigsuspend(_ctx) {
        log::error!("sigsuspend() unimplemented");
        0
    }
}

