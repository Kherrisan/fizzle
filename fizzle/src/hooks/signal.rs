use crate::hook_macros;
use crate::state;

hook_macros::hook! {
    unsafe fn sigwait(
        _set: *const libc::sigset_t,
        _sig: *mut libc::c_int
    ) -> libc::c_int => fizzle_sigwait(ctx) {
        // TODO: handle signals in the future
        drop(ctx);
        state::FIZZLE_STATE.yield_thread();

        libc::EINVAL
    }
}

hook_macros::hook! {
    unsafe fn sigwaitinfo(
        _set: *const libc::sigset_t,
        _info: *mut libc::siginfo_t
    ) -> libc::c_int => fizzle_sigwaitinfo(ctx) {

        drop(ctx);
        state::FIZZLE_STATE.yield_thread();

        libc::EINVAL
    }
}
