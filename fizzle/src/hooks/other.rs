use crate::hook_macros;

hook_macros::hook! {
    unsafe fn __ppc_get_timebase_freq(
    ) -> u64 => fizzle_ppc_get_timebase_freq(ctx) {
        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function __ppc_get_timebase_freq() called within signal handler")
            }           
        }

        hook_macros::real!(__ppc_get_timebase_freq)()
    }
}
