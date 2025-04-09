use crate::scheduler::FizzleSingleton;

pub mod aio;
pub mod dir;
pub mod entropy;
pub mod eventfd;
pub mod fanotify;
pub mod fd;
pub mod filestream;
pub mod filesystem;
pub mod inotify;
pub mod io;
pub mod io_uring;
pub mod mem;
pub mod netdb;
pub mod pipe;
pub mod poll;
pub mod posix_mq;
pub mod printf;
pub mod process;
pub mod pthread;
pub mod resolv;
pub mod scanf;
pub mod semaphore;
pub mod signal;
pub mod sleep;
pub mod socket;
pub mod syscall;
pub mod sysv_mq;
pub mod time;
pub mod uid;
pub mod xattr;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

pub static LOG_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn pre_hook() -> Option<FizzleSingleton> {
    if crate::state::has_entered_handler() {
        return None;
    }

    crate::state::set_entered_handler(true);

    if !LOG_INITIALIZED.fetch_or(true, Ordering::Relaxed) {
        // Initialize the logger to print the current PID/TID with each message
        env_logger::Builder::from_default_env()
            .format(|buf, record| {
                writeln!(
                    buf,
                    "[PID({})|{:?}|{}] {}",
                    std::process::id(),
                    std::thread::current().id(),
                    record.level().as_str().to_uppercase(),
                    record.args()
                )
            })
            .init();
        log::info!("Logger initialized");

        /*
        #[cfg(feature = "afl")]
        unsafe {
            
            if !matches!(std::env::var("FIZZLE_SINGLEPROCESS"), Ok(s) if s.as_str() == "1") {

                // These need to be called before __afl_manual_init().
                // However, when we use multiprocess shared memory (e.g. when the `AFL_SINGLEPROCESS`
                // environment variable isn't set) then __afl_manual_init() ends up being called
                // on the first invocation of any intercepted libc call, which naturally happens before
                // or during the first invocations to `__attribute(constructor)__` constructors.
                // We need to call it before initializing shared memory because __afl_manual_init()
                // defines where the forkserver will fork from, and shared memory must be fresh for
                // each new forked instance.
                //
                // The below functions are defined as constructor functions, which AFL takes as a
                // guarantee that they'll be called prior to __afl_manual_init(), but Fizzle violates
                // that assumption. Calling them here ensures this assumption is upheld.
                crate::__afl_auto_early();
                crate::__afl_auto_first();
                crate::__afl_auto_second();

            }
        }
        */
    }

    unsafe { Some(crate::scheduler::fizzle_singleton()) }
}

pub fn post_hook() {
    crate::state::set_entered_handler(false);
}
