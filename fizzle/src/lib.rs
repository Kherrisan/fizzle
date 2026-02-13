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
mod task;

pub(crate) use hook_macros::hook;

use critical_section::RawRestoreState;
use embedded_alloc::TlsfHeap;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, LinkedList, VecDeque};
use std::ffi::VaList;
use std::os::fd::RawFd;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

pub type GlobalRc<T> = Rc<RefCell<T>, GlobalHeap>;
pub type GlobalWeak<T> = std::rc::Weak<RefCell<T>, GlobalHeap>;

pub type GlobalList<T> = LinkedList<T, GlobalHeap>;
pub type GlobalDeque<T> = VecDeque<T, GlobalHeap>;
pub type GlobalVec<T> = Vec<T, GlobalHeap>;
pub type GlobalMap<K, V> = BTreeMap<K, V, GlobalHeap>;
pub type GlobalSet<K> = BTreeSet<K, GlobalHeap>;
pub type GlobalHashMap<K, V> = hashbrown::HashMap<K, V, hashbrown::DefaultHashBuilder, GlobalHeap>;
pub type GlobalBox<T> = Box<T, GlobalHeap>;
pub type GlobalHeap = &'static TlsfHeap;

unsafe extern "C" {
    #[cfg(feature = "afl")]
    pub fn __afl_manual_init();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_on();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_off();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_discard();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_skip();

    #[cfg(feature = "afl")]
    pub fn __afl_auto_early();

    #[cfg(feature = "afl")]
    pub fn __afl_auto_first();

    #[cfg(feature = "afl")]
    pub fn __afl_auto_second();

    #[cfg(feature = "pcr")]
    pub static __afl_already_initialized_second: u32;

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

    pub fn vsscanf(
        str: *const libc::c_char,
        format: *const libc::c_char,
        ap: VaList
    ) -> libc::c_int;

    pub fn res_mkquery(
        op: libc::c_int,
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        data: *const libc::c_uchar,
        datalen: libc::c_int,
        newrr: *const libc::c_uchar,
        buf: *mut libc::c_uchar,
        buflen: libc::c_int
    ) -> libc::c_int;

    static mut stdin: *mut libc::FILE;

    static mut stdout: *mut libc::FILE;

    static mut stderr: *mut libc::FILE;
}

static NEXT_WATCH_DESCRIPTOR: AtomicI32 = AtomicI32::new(0);

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

    unsafe fn release(_token: RawRestoreState) {
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

fn create_inotify_watch() -> libc::c_int {
    NEXT_WATCH_DESCRIPTOR.fetch_add(1, Ordering::Relaxed)
}

unsafe extern "C" fn fizzle_handle_sigchld(_signal: libc::c_int) {
    // TODO: BUG: this will not work with the current `has_entered_handler` implementation...
    let was_in_handler = crate::state::has_entered_handler();
    crate::state::set_entered_handler(true);

    loop {
        // Avoids zombie process buildup during persistent-mode fuzzing
        if libc::waitpid(-1, ptr::null_mut(), libc::WNOHANG) < 0 {
            crate::state::set_entered_handler(was_in_handler);

            return;
        }
    }
}

unsafe extern "C" fn fizzle_handle_term_signal(signum: libc::c_int) {
    // Kill all processes in the Fizzle harness (process group is always kept the same; changes by subprocesses are emulated.
    crate::state::set_entered_handler(true);

    // TODO: this is important for multi-processed programs, but it doesn't play well with AFL++...
    #[cfg(not(feature = "afl"))]
    libc::kill(0, signum);
    #[cfg(all(feature = "afl", feature = "quikcov"))]
    libc::exit(-signum); // Same value as SIGTERM sighandler; nullifies race condition
    #[cfg(all(feature = "afl", not(feature = "quikcov")))]
    libc::_exit(-signum); // Same value as SIGTERM sighandler; nullifies race condition
}

/// Utility for logging the `strace`-formatted output of each glibc call.
/// This is meant to make it easy for the strace log level to be raised/lowered as desired.
macro_rules! strace {
    // log_strace!(target: "my_target", key1 = 42, key2 = true; "a {} event", "log")
    // log_strace!(target: "my_target", "a {} event", "log")

    (target: $target:literal, $($arg:tt)+) => {
        let e = crate::errno::Errno::get_errno();
        log::log!(target: $target, log::Level::Trace, $($arg)+);
        e.set_errno();
    };

    // log_strace!("a {} event", "log")
    ($($arg:tt)+) => {
        let e = crate::errno::Errno::get_errno();
        log::log!(log::Level::Trace, $($arg)+);
        e.set_errno();
    }
}

pub(crate) use strace;
