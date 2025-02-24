#![feature(allocator_api)]
#![feature(btreemap_alloc)]
#![feature(c_variadic)]
#![feature(string_remove_matches)]

// extern crate libc;

mod backend;
mod cell;
mod comptime;
mod constants;
mod errno;
mod handlers;
mod hook_macros;
pub mod hooks;
mod plugins;
mod scheduler;
mod semaphore;
mod state;
mod streams;

use critical_section::RawRestoreState;
use embedded_alloc::TlsfHeap;
pub(crate) use hook_macros::hook;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, LinkedList};
use std::ffi::VaList;
use std::os::fd::RawFd;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub type GlobalRc<T> = Rc<RefCell<T>, GlobalHeap>;
pub type GlobalWeak<T> = std::rc::Weak<RefCell<T>, GlobalHeap>;

pub type GlobalList<T> = LinkedList<T, GlobalHeap>;
pub type GlobalVec<T> = Vec<T, GlobalHeap>;
pub type GlobalMap<K, V> = BTreeMap<K, V, GlobalHeap>;
pub type GlobalSet<K> = BTreeSet<K, GlobalHeap>;
pub type GlobalBox<T> = Box<T, GlobalHeap>;
pub type GlobalHeap = &'static TlsfHeap;

unsafe extern "C" {
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

    pub fn vasprintf(
        strp: *mut *mut libc::c_char,
        fmt: *const libc::c_char,
        ap: VaList,
    ) -> libc::c_int;

    static mut stdin: *mut libc::FILE;

    static mut stdout: *mut libc::FILE;

    static mut stderr: *mut libc::FILE;
}

// # SAFETY
//
// Defines a custom critical section that does not perform any mutex operations.
// This is meant to speed up allocation/deallocation procedures in Fizzle; it is
// safe so long as Fizzle accurately ensures that only one thread is executing at
// a given time.
struct MyCriticalSection;
critical_section::set_impl!(MyCriticalSection);

unsafe impl critical_section::Impl for MyCriticalSection {
    unsafe fn acquire() -> RawRestoreState {
        // no-op
    }

    unsafe fn release(token: RawRestoreState) {
        // no-op
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WaitDuration {
    /// Returns immediately if the event is not ready
    Immediate,
    /// Waits for the given amount of time, returning ETIMEDOUT if the event isn't ready.
    Timed(Duration),
    /// Waits indefinitely until the semaphore can be acquired.
    Indefinite,
}

fn afl_onetime_init() {
    static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

    if !IS_INITIALIZED.fetch_or(true, Ordering::Relaxed) {
        #[cfg(feature = "pcr")]
        unsafe {
            crate::__afl_sharedmem_fuzzing = 1;
        }

        log::debug!("calling __afl_manual_init()");
        unsafe {
            crate::__afl_manual_init();
        }
        log::debug!("__afl_manual_init finished");
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

fn create_descriptor() -> RawFd {
    // For some reason using `memfd_create` here lead to UB and the function would (erroneously)
    // always return 0. Using `eventfd` instead seems to fix this.
    let fd = unsafe { libc::eventfd(0, 0) };
    if fd < 0 {
        panic!("fizzle internal file descriptor alias creation (`memfd_create`) failed");
    }
    fd
}

fn destroy_descriptor(fd: RawFd) {
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
