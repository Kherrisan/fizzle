#![feature(c_variadic)]
#![feature(new_uninit)]

#![feature(string_remove_matches)]

extern crate libc;

mod arena;
mod backend;
mod comptime;
mod constants;
mod handlers;
mod hook_macros;
mod once;
mod plugins;
mod semaphore;
mod state;
mod streams;
pub mod hooks;

pub(crate) use hook_macros::hook;

use std::os::fd::RawFd;
use std::ptr;

extern "C" {
    #[cfg(feature = "afl")]
    pub fn __afl_manual_init();

    // TODO: three underscores for Apple
    #[cfg(feature = "pcr")]
    pub fn __afl_persistent_loop(input: libc::c_uint) -> libc::c_int;

    #[cfg(feature = "pcr")]
    pub static __afl_fuzz_len: *mut libc::c_uint;

    #[cfg(feature = "pcr")]
    pub static __afl_fuzz_ptr: *mut libc::c_uchar;

    #[cfg(feature = "pcr")]
    pub static __afl_connected: libc::c_int;

    #[cfg(feature = "pcr")]
    pub static mut __afl_sharedmem_fuzzing: libc::c_int;

    static mut stdin: *mut libc::FILE;

    static mut stdout: *mut libc::FILE;

    static mut stderr: *mut libc::FILE;
}

pub fn report_strict_failure(explanation: &'static str) {
    debug_assert!(false, "{}", explanation);
    log::error!("{}", explanation);
}

/// Converts the given errno value into string representation.
/// 
/// This function acts like `strerrorname_np`, except without the race conditions or platform incompatibilities.
fn errno_str() -> &'static str {
    let errno = unsafe { *libc::__errno_location() };
    match errno {
        libc::E2BIG => "E2BIG",
        libc::EACCES => "EACCESS",
        libc::EADDRINUSE => "EADDRINUSE",
        libc::EADDRNOTAVAIL => "EADDRNOTAVAIL",
        libc::EAFNOSUPPORT => "EAFNOSUPPORT",
        libc::EAGAIN => "EAGAIN",
        libc::EALREADY => "EALREADY",
        libc::EBADE => "EBADE",
        libc::EBADF => "EBADF",
        libc::EBADFD => "EBADFD",
        libc::EBADMSG => "EBADMSG",
        libc::EBADR => "EBADR",
        libc::EBADRQC => "EBADRQC",
        libc::EINVAL => "EINVAL",
        libc::EMFILE => "EMFILE",
        libc::ENFILE => "ENFILE",
        libc::ENOBUFS => "ENOBUFS",
        libc::ENOMEM => "ENOMEM",
        libc::EPROTONOSUPPORT => "EPROTONOSUPPORT",
        _ => panic!("Fizzle internal error: add errno string for errno number {}", errno),
    }
}

/// Creates a new location in memory that is guaranteed to be unique to others.
/// This is particularly useful in emulating hooks that require a pointer as a return value.
/// Memory locations should be destroyed with `unique_mem_destroy()` once finished using.
unsafe fn unique_mem_create() -> *mut libc::c_void {
    // TODO: turn this into an alias creator that uses sequential addresses in allocated to handle these opaque references more efficiently.

    let addr = libc::mmap(
        ptr::null_mut(),
        1,
        libc::PROT_NONE,
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
        -1,
        0,
    );
    if addr.is_null() {
        panic!("failed to create unique memory handle via `mmap`");
    }

    addr
}

/// Unmaps a location in memory created with `unique_mem_create()`.
/// This uses `munmap` under the hood; it is unsafe to call this on any `mem_location` other than those returned by `unique_mem_create()`.
unsafe fn unique_mem_destroy(mem_location: *mut libc::c_void) {
    let res = unsafe { libc::munmap(mem_location, 1) };
    if res != 0 {
        panic!("error during destruction of unique memory handle via `mmap`");
    }
}

fn alias_fd_create() -> RawFd {
    let fd = unsafe { libc::memfd_create(c"FIZZLE_ALIAS_FD".as_ptr(), 0) };
    if fd < 0 {
        panic!("fizzle internal file descriptor alias creation (`memfd_create`) failed");
    }
    fd
}

fn alias_fd_destroy(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}

/// Utility for logging the `strace`-formatted output of each glibc call.
/// This is meant to make it easy for the strace log level to be raised/lowered as desired.
macro_rules! strace {
    // log_strace!(target: "my_target", key1 = 42, key2 = true; "a {} event", "log")
    // log_strace!(target: "my_target", "a {} event", "log")
    (target: $target:expr, $($arg:tt)+) => (log::log!(target: $target, log::Level::Info, $($arg)+));

    // log_strace!("a {} event", "log")
    ($($arg:tt)+) => (log::log!(log::Level::Info, $($arg)+))
}

pub(crate) use strace;
