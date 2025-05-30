use std::ffi::CStr;

use crate::handlers::env::GetEnvEvent;
use crate::hook_macros;
use crate::scheduler::Scheduler;

// We need this because it's the *only* libc function that precedes some of AFL's persistent-mode initialization.
// We need to have __afl_sharedmem_fuzzing = 1 be applied *before* afl_auto_early() checks it, and that function
// happens to call a setenv before checking the variable.

hook_macros::hook! {
    unsafe fn getenv(name: *const libc::c_char) -> *mut libc::c_char => fizzle_getenv(ctx) {
        let name_cstr = CStr::from_ptr(name);
        crate::strace!("getenv({:?}) -> ...", name_cstr);

        match Scheduler::handle_event(&mut ctx, GetEnvEvent::new(name_cstr)) {
            Ok(ptr) => {
                if ptr.is_null() {
                    crate::strace!("getenv({:?}) -> NULL", name_cstr);
                } else {
                    crate::strace!("getenv({:?}) -> {:?}", name_cstr, CStr::from_ptr(ptr));
                }

                ptr
            },
            Err(()) => unreachable!(),
        }
    }
}
