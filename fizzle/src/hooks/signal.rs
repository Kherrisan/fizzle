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
