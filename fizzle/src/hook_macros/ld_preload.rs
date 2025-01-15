// Uses code from `redhook` project, available under BSD 2-Clause License

#[link(name = "dl")]
unsafe extern "C" {
    unsafe fn dlsym(handle: *const libc::c_void, symbol: *const libc::c_char) -> *const libc::c_void;
}

pub unsafe fn dlsym_next(symbol: &'static str) -> *const u8 {
    let ptr = dlsym(libc::RTLD_NEXT, symbol.as_ptr().cast::<libc::c_char>());
    assert!(!ptr.is_null(), "dlsym: unable to find underlying function for {}", symbol);
    ptr.cast::<u8>()
}

macro_rules! hook {
    (unsafe fn $real_fn:ident ( $($v:ident : $t:ty),* ) -> $r:ty => $hook_fn:ident ( $state:ident ) $body:block) => {
        #[allow(non_camel_case_types)]
        pub struct $real_fn {__private_field: ()}
        #[allow(non_upper_case_globals)]
        static $real_fn: $real_fn = $real_fn {__private_field: ()};

        impl $real_fn {
            fn get(&self) -> unsafe extern fn ( $($v : $t),* ) -> $r {
                use std::cell::OnceCell;

                std::thread_local! {
                    static REAL: OnceCell<*const u8> = OnceCell::new();
                }

                unsafe {
                    std::mem::transmute(REAL.with(|cell| {
                        *cell.get_or_init(|| {
                            crate::hook_macros::ld_preload::dlsym_next(concat!(stringify!($real_fn), "\0"))
                        })
                    }))
                }
            }

            #[no_mangle]
            pub unsafe extern "C" fn $real_fn ( $($v : $t),* ) -> $r {
                ::std::panic::catch_unwind(|| {
                    if crate::state::has_entered_handler() {
                        return $real_fn.get() ( $($v),* )
                    }
                    
                    crate::state::set_entered_handler(true);
                    let res = {
                        $hook_fn ( $($v),*)
                    };
                    crate::state::set_entered_handler(false);

                    res

                }).unwrap_or_else(|_| {
                    std::process::abort(); // Panic unwind hook already prints out stack info
                })
            }
        }

        pub unsafe fn $hook_fn ( $($v : $t),*) -> $r {
            #[allow(unused_mut)]
            let mut $state = unsafe {
                crate::scheduler::fizzle_singleton()
            };

            $body
        }
    };

    // Handle case where function signature has no return type
    (unsafe fn $real_fn:ident ( $($v:ident : $t:ty),* ) => $hook_fn:ident ( $state:ident ) $body:block) => {
        $crate::hook! { unsafe fn $real_fn ( $($v : $t),* ) -> () => $hook_fn ( $state ) $body }
    };
}

use std::cell::OnceCell;

pub(crate) use hook;

macro_rules! real {
    ($real_fn:ident) => {
        $real_fn.get()
    };
}

pub(crate) use real;

pub fn real_syscall() -> extern "C" fn(libc::c_long, ...) -> libc::c_long {
    std::thread_local! {
        static REAL: OnceCell<*const u8> = OnceCell::new();
    }

    unsafe {
        std::mem::transmute(REAL.with(|cell| {
            *cell.get_or_init(|| {
                dlsym_next("syscall\0")
            })
        }))
    }
}

/*
pub fn real_fcntl() -> extern "C" fn(libc::c_long, ...) -> libc::c_long {
    std::thread_local! {
        static REAL: OnceCell<*const u8> = OnceCell::new();
    }

    unsafe {
        std::mem::transmute(REAL.with(|cell| {
            *cell.get_or_init(|| {
                dlsym_next("syscall\0")
            })
        }))
    }
}
*/
