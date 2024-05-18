// Uses code from `redhook` project, available under BSD 2-Clause License

use libc::{c_char, c_void};

#[link(name = "dl")]
extern "C" {
    fn dlsym(handle: *const c_void, symbol: *const c_char) -> *const c_void;
}

const RTLD_NEXT: *const c_void = -1isize as *const c_void;

pub unsafe fn dlsym_next(symbol: &'static str) -> *const u8 {
    let ptr = dlsym(RTLD_NEXT, symbol.as_ptr() as *const c_char);
    if ptr.is_null() {
        panic!(
            "LD_PRELOAD: Unable to find underlying function for {}",
            symbol
        );
    }
    ptr as *const u8
}

macro_rules! hook {
    (unsafe fn $real_fn:ident ( $($v:ident : $t:ty),* ) -> $r:ty => $hook_fn:ident ( $state:ident ) $body:block) => {
        #[allow(non_camel_case_types)]
        pub struct $real_fn {__private_field: ()}
        #[allow(non_upper_case_globals)]
        static $real_fn: $real_fn = $real_fn {__private_field: ()};

        impl $real_fn {
            fn get(&self) -> unsafe extern fn ( $($v : $t),* ) -> $r {
                use ::std::sync::Once;

                static mut REAL: *const u8 = 0 as *const u8;
                static mut ONCE: Once = Once::new();

                unsafe {
                    ONCE.call_once(|| {
                        REAL = $crate::hook_macros::ld_preload::dlsym_next(concat!(stringify!($real_fn), "\0"));
                    });
                    ::std::mem::transmute(REAL)
                }
            }

            #[no_mangle]
            pub unsafe extern fn $real_fn ( $($v : $t),* ) -> $r {
                ::std::panic::catch_unwind(|| {
                    if crate::state::has_entered_handler() {
                        // Use actual function instead of fizzle
                        return $real_fn.get() ( $($v),* )
                    }
                    crate::state::set_entered_handler(true);
                    crate::trace_enter!($real_fn);
                    let res = {

                        $hook_fn ( $($v),*)
                    };
                    // crate::trace_exit!($real_fn);
                    crate::state::set_entered_handler(false);
                    res
                }).unwrap_or_else(|e| {
                    eprintln!("fizzle hook panicked: {:?}", e);
                    std::process::abort();
                })
            }
        }

        pub unsafe fn $hook_fn ( $($v : $t),*) -> $r {
            let $state = state::get_fizzle_state();
            $body
        }
    };

    // Handle case where function signature has no return type
    (unsafe fn $real_fn:ident ( $($v:ident : $t:ty),* ) => $hook_fn:ident ( $state:ident ) $body:block) => {
        $crate::hook! { unsafe fn $real_fn ( $($v : $t),* ) -> () => $hook_fn ( $state ) $body }
    };
}

pub(crate) use hook;

macro_rules! real {
    ($real_fn:ident) => {
        $real_fn.get()
    };
}

pub(crate) use real;
