
pub mod fd;
pub mod ipc;

use std::{array, env, mem, thread};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString};
use std::hash::{Hash, Hasher};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::{RefCell, UnsafeCell};
use std::thread::ThreadId;

use heapless::spsc::Queue;

use fxhash::FxBuildHasher;

use crate::semaphore::Semaphore;
use crate::FilePath;

use self::fd::FdInfo;
use self::ipc::IpcMemory;

const FIZZLE_MEMORY_ENV: &'static CStr = c"FIZZLE_MEMORY";

const FIZZLE_MAX_READY_PROCESSES: usize = 256;
const FIZZLE_MAX_THREADS: usize = 65536;

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = RefCell::new(false);
}

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

// static OLD_STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// State that remains synchronized across multiple processes via shared memory.
// static GLOBAL_STATE: OnceLock<IpcMemory<GlobalState>> = OnceLock::new();

static FIZZLE_STATE: FizzleOnce = FizzleOnce::new();


/// Indicates whether the thread is currently executing within a fizzle handler.
/// 
/// We want to be able to call rust functions that may use syscalls without those leading to 
/// infinite recursion. To do so, we keep track of whether we've already hooked the current
/// function using a thread-local variable.
pub fn has_entered_handler() -> bool {
    let mut entered = true;
    ENTERED_HANDLER.with(|e| { entered = *e.borrow(); });
    entered
}

pub fn set_entered_handler(entered: bool) {
    ENTERED_HANDLER.with(|e| { *e.borrow_mut() = entered; });
}

pub fn fizzle_trace_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed)
}

/// ================================================================================================
///                                       PUBLIC FUNCTIONS
/// ================================================================================================


/// A global singleton storing fizzle data and state.
/// 
struct FizzleOnce {
    inner: UnsafeCell<Option<FizzleState>>, 
}

unsafe impl Send for FizzleOnce {}
unsafe impl Sync for FizzleOnce {}

impl FizzleOnce {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }

    /// Retrieves a mutable reference to global fizzle state.
    /// 
    /// # Safety
    /// 
    /// This method returns a global mutable variable without any explicit mutex/semaphore guarding
    /// it. As such, accessing state ([`ProcessState`] or [`InterprocessState`]) via this method is
    /// **only** safe when no two threads/processes access or mutate that state at the same time.
    /// 
    /// Fizzle accomplishes this by: a) hooking thread/process creation (`pthread_create`, `fork`),
    /// and b) enforcing that a thread never accesses process/interprocess state variables from the
    /// time it delegates execution to another thread until it is delegated execution back (i.e.,
    /// within  [yield_thread()](FizzleState::yield_thread)).
    /// 
    /// a) is accomplished within the `hooks/pthread` module.
    /// 
    /// b) is accomplished by having [`yield_thread()`](FizzleState::yield_thread) and 
    /// [`process_state()`](FizzleState::process_state)/[`interprocess_state()`](FizzleState::interprocess_state)
    /// both require mutable references to [`FizzleState`]. This ensures state will never be held
    /// at the time a thread is yielded. Process/interprocess state is never accessed within
    /// `yield_thread` (other than for a few statically-allocated semaphores).
    /// 
    /// Lastly, the `FizzleOnce` singleton variable [`STATE`] is kept local to this module and is
    /// never accessed other than via [`get_fizzle_state()`], which in turn is called as part of the
    /// libc hook macro.
    /// 
    fn get(&self) -> &mut FizzleState {
        let inner = unsafe { &mut *self.inner.get() };
        match inner {
            Some(state) => state,
            None => fizzle_state_initialize(inner),
        }
    }
}

// Labelling this as cold and not inlining ensures the `None` branch will be marked cold
#[cold]
#[inline(never)]
fn fizzle_state_initialize(state: &mut Option<FizzleState>) -> &mut FizzleState {
    state.insert(FizzleState::new())
}

/// Retrieves the current fizzle state.
///
/// SAFETY: this function MUST ONLY be called within `ld_preload.rs` or `dyld_insert_libraries.rs`.
/// Accessing the global `FizzleState` variable multiple times in one scope will lead to UB.
pub fn get_fizzle_state() -> &'static mut FizzleState {
    FIZZLE_STATE.get()
}

/// The collective process/interprocess state that fizzle has global access to.
/// 
pub struct FizzleState {
    thread_locks: [Option<Semaphore>; FIZZLE_MAX_THREADS],
    /// `local`, as in local to the current executing process.
    local: ProcessState,
    /// `global`, as in shared across all processes in a given fizzle harness.
    global: IpcMemory<InterprocessState>,
}

impl FizzleState {
    /// Initializes the fizzle state.
    /// 
    /// This will be called when the first shimmed libc call is executed--only one thread
    /// is executing at the time, and no libc calls have completed yet.
    fn new() -> Self {
        let mut thread_locks = array::from_fn(|_| None);

        // Initialize the lock for the current thread
        let thread_idx = index_of_thread(&thread::current().id());
        assert!(thread_idx == 0, "unexpected ThreadId value `{}` on fizzle startup", thread_idx);
        thread_locks[thread_idx] = Some(Semaphore::new(0));

        let mem_location_ptr = unsafe { libc::getenv(FIZZLE_MEMORY_ENV.as_ptr()) };
        
        if mem_location_ptr.is_null() {
            let global = IpcMemory::new(InterprocessState::new());
            let process_id = ProcessId::new(0);

            Self {
                thread_locks,
                local: ProcessState::new(process_id),
                global,
            }
        } else {
            let mem_location = unsafe { CStr::from_ptr(mem_location_ptr) };
            let mut global: IpcMemory<InterprocessState> = IpcMemory::from_identifier(mem_location);
            let process_id = global.data().assign_process_id();

            Self {
                thread_locks,
                local: ProcessState::new(process_id),
                global,
            }
        }
    }

    /// Retrieve state specific to the process.
    pub fn local(&mut self) -> &mut ProcessState {
        &mut self.local
    }

    /// Retrieve state shared across processes.
    pub fn global(&mut self) -> &mut InterprocessState {
        self.global.data()
    }

    /// Pauses execution of the current thread and delegates control flow to another thread/process.
    /// Once all threads/processes have finished executing, this returns control flow to the primary
    /// fuzzing process, which signals to the fuzzer that it is ready for the next input.
    pub fn yield_thread(&mut self) {

        // Check to see if all threads have finished execution for this process
        if let Some(thread_id) = self.local.ready_threads.pop_front() {
            // ...if not, then run the next one.
            self.get_thread_lock(&thread_id).post();

            // Pause our current thread until it gets delegated execution again.
            self.get_thread_lock(&thread::current().id()).wait();
        } else {
            // ...if all threads have finished execution, move to next process.
            if let Some(next_process_id) = self.global().ready_processes.dequeue() {
                self.global.process_wake(next_process_id);

                // Wait for a process to delegate back to this one.
                let process_id = self.local().process_id;
                self.global.process_wait(process_id);
            }else {
                // If no ready processes are left, notify the fuzzing engine
                self.notify_complete()
            };
        }

        // Thread ready to execute
    }

    /// Notifies the fuzzing engine that the current round of fuzzing has finished.
    /// Note that 
    fn notify_complete(&mut self) {
        // Communicate that process is finished running

        // Wait for input from the fuzzing engine...

        // Mark appropriate processes/threads as ready to receive input

        // If our thread isn't receiving input, continue on to the next.
        if false {
            // This will not recurse infinitely, as we mark threads ready for input just before it.
            self.yield_thread();
        }
    }

    /// Ceases execution of the current thread.
    pub fn exit_thread(&mut self, ret: *mut libc::c_void) -> ! {
        let thread_id = thread::current().id();

        // Mark this thread as dead
        self.local.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = self.local.awaiting_thread_death.remove(&thread_id) {
            for awaiting_id in awaiting_threads {
                self.local.ready_threads.push_back(awaiting_id);
            }
        }

        // De-allocate the lock for this thread
        let mut sem: Option<Semaphore> = None;
        mem::swap(&mut sem, &mut self.thread_locks[index_of_thread(&thread_id)]);
        unsafe { sem.unwrap().destroy() };

        // Finally, exit the thread properly
        unsafe { libc::pthread_exit(ret) }
    }

    /// Retrieves the semaphore associated with the thread idendified by `thread_id`.
    fn get_thread_lock(&self, thread_id: &ThreadId) -> &Semaphore {
        let thread_idx = index_of_thread(thread_id);
        assert!(thread_idx < self.thread_locks.len(), "too many threads spawned during fizzle execution (ThreadID out of range)--increase FIZZLE_MAX_THREADS constant during fizzle compilation");
        self.thread_locks[thread_idx].as_ref().unwrap()
    }
}

/// State local to the current process.
pub struct ProcessState {
    pub process_id: ProcessId,
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
    pub pthreads: HashMap<libc::pthread_t, ThreadId, FxBuildHasher>,
    pub ready_threads: VecDeque<ThreadId>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    pub debug_enabled: bool,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath,
}

impl ProcessState {
    fn new(process_id: ProcessId) -> Self {
        let debug_enabled = match env::var("FIZZLE_DEBUG") {
            Ok(s) if s.as_str() == "1" => true,
            _ => false,
        };

        let trace_enabled = match env::var("FIZZLE_TRACE") {
            Ok(s) if s.as_str() == "1" => true,
            _ => false,
        };

        TRACE_ENABLED.store(trace_enabled, Ordering::Release);


        let working_directory_bytes = std::env::current_dir().map(|dir| FilePath::from_raw_bytes(dir.as_os_str().as_bytes()).unwrap_or_default());
        let working_directory = working_directory_bytes.unwrap_or_default();

        Self {
            process_id, // TODO: increment each time new process is made
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
            debug_enabled,
            working_directory,
            awaiting_thread_death: HashMap::with_hasher(Default::default()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId {
    identifier: usize,
}

impl ProcessId {
    pub fn new(ident: usize) -> Self {
        Self {
            identifier: ident,
        }
    }

    pub fn ident(&self) -> usize {
        self.identifier
    }
}

/// State/data shared among all processes in a fizzle execution.
pub struct InterprocessState {
    /// The next process ID available to be assigned to a new process.
    next_process_id: ProcessId,
    ready_processes: Queue<ProcessId, FIZZLE_MAX_READY_PROCESSES>,
}

impl InterprocessState {
    fn new() -> Self {
        Self {
            next_process_id: ProcessId::new(1), // First process takes 0, so next is 1
            ready_processes: Queue::new(),
        }
    }

    /// Assigns the next available process ID and increments it internally.
    fn assign_process_id(&mut self) -> ProcessId {
        let process_id = self.next_process_id;
        self.next_process_id.identifier += 1;
        process_id
    }
}

/// A hasher that correctly outputs the internal value of a [`ThreadId`] for its hash.
pub struct ThreadHasher {
    value: u64,
}

impl ThreadHasher {
    pub fn new() -> Self {
        Self {
            value: 0,
        }
    }
}

impl Hasher for ThreadHasher {
    fn finish(&self) -> u64 {
        self.value
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut idx = 0usize;
        while bytes.len() - idx >= 8 {
            let bytearray: [u8; 8] = bytes[idx..idx + 8].try_into().unwrap();
            self.value += u64::from_le_bytes(bytearray);
            idx += 8;
        }

        if idx != bytes.len() {
            let mut bytearray = [0u8; 8];
            for (i, b) in bytes[idx..].iter().rev().enumerate() {
                bytearray[i] = *b;
            }
            self.value += u64::from_le_bytes(bytearray);
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.value += i as u64;
    }

    fn write_u64(&mut self, i: u64) {
        self.value += i as u64;
    }
}

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

// ---=== Helper Functions ===---

fn index_of_thread(thread: &ThreadId) -> usize {
    let mut hasher = ThreadHasher::new();
    thread.hash(&mut hasher);
    hasher.finish() as usize
}
