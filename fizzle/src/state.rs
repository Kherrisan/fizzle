
pub mod fd;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CString;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{env, thread};
use std::sync::{OnceLock, Mutex};
use std::cell::RefCell;
use std::thread::{Thread, ThreadId};

use fxhash::FxBuildHasher;
use libc::pthread_t;

use crate::FilePath;

use self::fd::FdInfo;

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = RefCell::new(false);
}

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

static FIZZLE_STATE: OnceLock<Mutex<State>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BarrierId(usize);

impl From<*mut libc::pthread_barrier_t> for BarrierId {
    fn from(value: *mut libc::pthread_barrier_t) -> Self {
        BarrierId(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CondVarId(usize);

impl From<*mut libc::pthread_cond_t> for CondVarId {
    fn from(value: *mut libc::pthread_cond_t) -> Self {
        CondVarId(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutexId(usize);

impl From<*mut libc::pthread_mutex_t> for MutexId {
    fn from(value: *mut libc::pthread_mutex_t) -> Self {
        MutexId(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RwLockId(usize);

impl From<*mut libc::pthread_rwlock_t> for RwLockId {
    fn from(value: *mut libc::pthread_rwlock_t) -> Self {
        RwLockId(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpinlockId(usize);

impl From<*mut libc::pthread_spinlock_t> for SpinlockId {
    fn from(value: *mut libc::pthread_spinlock_t) -> Self {
        SpinlockId(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemaphoreId(usize);

impl From<*mut libc::sem_t> for SemaphoreId {
    fn from(value: *mut libc::sem_t) -> Self {
        SemaphoreId(value as usize)
    }
}

impl SemaphoreId {
    pub(crate) fn to_mut_ptr(self) -> *mut libc::sem_t {
        self.0 as *mut libc::sem_t
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(usize);

impl From<*mut libc::FILE> for FileId {
    fn from(value: *mut libc::FILE) -> Self {
        FileId(value as usize)
    }
}

#[derive(Debug)]
pub struct BarrierInfo {
    pub curr: Vec<ThreadId>,
    pub needed: usize,
}

#[derive(Debug)]
pub struct FileInfo {
    pub temporary: bool,
}

impl FileInfo {
    /// Creates a new temporary file.
    pub fn new() -> Self {
        Self {
            temporary: true,
        }
    }
}

/*
#[derive(Debug)]
pub enum FileMode {
    Readonly,
    Writeonly,
    ReadWrite,
}
*/

#[derive(Debug)]
pub struct RwLockInfo {
    pub state: RwLockState,
    pub awaiting_read: VecDeque<ThreadId>,
    pub awaiting_write: VecDeque<ThreadId>,
    pub holding_state: HashSet<ThreadId, FxBuildHasher>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RwLockState {
    Available,
    Reading,
    Writing,
}

#[derive(Debug)]
pub struct SemaphoreInfo {
    pub name: Option<CString>,
    pub value: usize,
    pub waiting: VecDeque<ThreadId>,
}

#[derive(Debug)]
pub struct ThreadInfo {
    /// The handle of the given thread.
    pub thread: Thread,
    /// If set, indicates that execution has been delegated to the current thread.
    pub delegated: bool,
}

/// Global singleton that holds (nearly) all internal information for fizzle.
#[derive(Debug)]
pub struct State {
    pub fds: HashMap<RawFd, FdInfo, FxBuildHasher>,
    pub barriers: HashMap<BarrierId, BarrierInfo, FxBuildHasher>,
    pub condvars: HashMap<CondVarId, VecDeque<ThreadId>, FxBuildHasher>,
    /// Files specifically designated as being emulated.
    pub files: HashMap<FilePath, FileInfo, FxBuildHasher>,
    pub file_objs: HashMap<FileId, RawFd, FxBuildHasher>,
    pub passthrough_file_objs: HashMap<FileId, RawFd>,
    pub mutexes: HashMap<MutexId, VecDeque<ThreadId>, FxBuildHasher>,
    pub named_semaphores: HashMap<CString, SemaphoreId>,
    pub rwlocks: HashMap<RwLockId, RwLockInfo, FxBuildHasher>,
    pub semaphores: HashMap<SemaphoreId, SemaphoreInfo>,
    pub spinlocks: HashMap<SpinlockId, VecDeque<ThreadId>, FxBuildHasher>,
    pub pthreads: HashMap<pthread_t, ThreadId, FxBuildHasher>,
    pub ready_threads: VecDeque<ThreadId>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    pub program_threads: HashMap<ThreadId, ThreadInfo, FxBuildHasher>,
    pub debug_enabled: bool,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath,
}

impl State {
    fn new(debug_enabled: bool) -> Self {
        let working_directory_bytes = std::env::current_dir().map(|dir| FilePath::from_raw_bytes(dir.as_os_str().as_bytes()).unwrap_or_default());
        let working_directory = working_directory_bytes.unwrap_or_default();

        Self {
            barriers: HashMap::with_hasher(Default::default()),
            condvars: HashMap::with_hasher(Default::default()),
            files: HashMap::with_hasher(Default::default()),
            fds: HashMap::with_hasher(Default::default()),
            file_objs: HashMap::with_hasher(Default::default()),
            passthrough_file_objs: HashMap::with_hasher(Default::default()),
            mutexes: HashMap::with_hasher(Default::default()),
            named_semaphores: HashMap::with_hasher(Default::default()),
            rwlocks: HashMap::with_hasher(Default::default()),
            semaphores: HashMap::with_hasher(Default::default()),
            spinlocks: HashMap::with_hasher(Default::default()),
            pthreads: HashMap::with_hasher(Default::default()),
            ready_threads: VecDeque::new(),
            terminated_threads: HashSet::with_hasher(Default::default()),
            program_threads: HashMap::with_hasher(Default::default()),
            debug_enabled,
            working_directory,
            awaiting_thread_death: HashMap::with_hasher(Default::default()),
        }
    }

    /// Checks to see if the current thread has been flagged as started, and consumes the flag.
    pub fn thread_delegated(&mut self) -> bool {
        match self.program_threads.get_mut(&thread::current().id()) {
            None => false,
            Some(thread_info) => if thread_info.delegated {
                thread_info.delegated = false; // Consume started flag
                true
            } else {
                false
            }
        }
    }

    /// Mark the currently-running thread as exiting execution.
    pub fn exit_current_thread(&mut self) {
        let thread_id = thread::current().id();

        self.terminated_threads.insert(thread_id);

        if let Some(awaiting_threads) = self.awaiting_thread_death.remove(&thread_id) {
            for awaiting_id in awaiting_threads {
                self.ready_threads.push_back(awaiting_id);
            }

            self.program_threads.remove(&thread_id).unwrap();
        }
    }

    /// Start execution of the next available thread
    pub fn wake_next_thread(&mut self) {
        if let Some(thread_id) = self.ready_threads.pop_front() {
            let thread_info = self.program_threads.get_mut(&thread_id).unwrap();
            thread_info.delegated = true;
            thread_info.thread.unpark();
        } else {
            // Deadlock condition: just let all threads block so that the fuzzer times out
        }
    }
}

#[inline]
pub fn fizzle_trace_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed)
}

#[inline]
pub fn fizzle_state() -> &'static Mutex<State> {
    FIZZLE_STATE.get().unwrap()
}

#[inline]
pub fn fizzle_initialize() {
    if FIZZLE_STATE.get().is_none() {
        fizzle_initialize_once();
    }
}

/// Indicates whether the thread is currently executing within a fizzle handler.
/// 
/// We want to be able to call rust functions that may use syscalls without those leading to 
/// infinite recursion. To do so, we keep track of whether we've already hooked the current
/// function using a thread-local variable.
#[inline]
pub fn has_entered_handler() -> bool {
    let mut entered = true;
    ENTERED_HANDLER.with(|e| { entered = *e.borrow(); });
    entered
}

#[inline]
pub fn set_entered_handler(entered: bool) {
    ENTERED_HANDLER.with(|e| { *e.borrow_mut() = entered; });
}

/// Initializes all global variables and the `fizzle` scheduler thread.
/// 
/// This method is called when the first hooked system call is called by the program (which is most
/// likely to be `read()` from DLL loading).
/// 
/// # Thread Safety
/// This is not a thread-safe method; simultaneous calls to it across threads will lead to deadlock.
/// 
/// This method is only called when the application is in a single-threaded context, as
/// any attempt by the program to make more than one thread (`pthread_create`) will be hooked to contain `fizzle_initialize` first.

#[cold]
#[inline(never)]
fn fizzle_initialize_once() {
    let debug_enabled = match env::var("FIZZLE_DEBUG") {
        Ok(s) if s.as_str() == "1" => true,
        _ => false,
    };

    let trace_enabled = match env::var("FIZZLE_TRACE") {
        Ok(s) if s.as_str() == "1" => true,
        _ => false,
    };

    TRACE_ENABLED.store(trace_enabled, Ordering::Release);

    FIZZLE_STATE.set(Mutex::new(State::new(debug_enabled))).unwrap();

    let mut state = fizzle_state().lock().unwrap();
    state.program_threads.insert(thread::current().id(), ThreadInfo {
        thread: thread::current(),
        delegated: true,
    });

    state.pthreads.insert(unsafe { libc::pthread_self() }, thread::current().id());
}


