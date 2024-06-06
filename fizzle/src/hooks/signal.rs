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
