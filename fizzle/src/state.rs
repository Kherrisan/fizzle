pub mod fd;
pub mod ipc;

use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CStr;
use std::hash::{Hash, Hasher};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::{array, env, mem, thread};

use heapless::spsc::Queue;

use fxhash::FxBuildHasher;
use heapless::{Deque, FnvIndexMap};

use crate::semaphore::Semaphore;
use crate::{FilePath, RingBuffer, SemPath, ValueIndex};

use self::fd::FdInfo;
use self::ipc::IpcMemory;

const FIZZLE_MEMORY_ENV: &CStr = c"FIZZLE_MEMORY";

const FIZZLE_MAX_READY_PROCESSES: usize = 256;
const FIZZLE_MAX_THREADS: usize = 65536;

/// The maximum number of paths to files fizzle emulates.
const FIZZLE_MAX_FILE_PATHS: usize = 512;
/// The maximum number of files fizzle can emulate.
const FIZZLE_MAX_FILES: usize = 512;

const FIZZLE_MAX_DIRS: usize = 256;

const FIZZLE_MAX_PIPES: usize = 256;

const FIZZLE_MAX_MESSAGE_QUEUES: usize = 256;

const FIZZLE_BUFFER_LENGTH: usize = 262_144; // 256 KB per buffer (twice the Linux default for `/proc/sys/net/ipv4/tcp_rmem`)

const FIZZLE_MAX_BUFFERS: usize = 256; // 256 * 128 KB = 64 MB total

const FIZZLE_MAX_SOCKETS: usize = 256;

const FIZZLE_MAX_NAMED_SEMAPHORES: usize = 128;

const FIZZLE_MAX_FDS: usize = 4096;

const FIZZLE_MAX_WAITING_SEMAPHORES: usize = 32;

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = const { RefCell::new(false) };
}

static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

// static OLD_STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// State that remains synchronized across multiple processes via shared memory.
// static GLOBAL_STATE: OnceLock<IpcMemory<GlobalState>> = OnceLock::new();

/// Retrieves the current fizzle state.
///
/// SAFETY: this function MUST ONLY be called within `ld_preload.rs` or `dyld_insert_libraries.rs`.
/// Accessing the global `FizzleState` variable multiple times in one scope will lead to UB.
pub static FIZZLE_STATE: FizzleCell = FizzleCell::new();

/// Indicates whether the thread is currently executing within a fizzle handler.
///
/// We want to be able to call rust functions that may use syscalls without those leading to
/// infinite recursion. To do so, we keep track of whether we've already hooked the current
/// function using a thread-local variable.
pub fn has_entered_handler() -> bool {
    let mut entered = true;
    ENTERED_HANDLER.with(|e| {
        entered = *e.borrow();
    });
    entered
}

pub fn set_entered_handler(entered: bool) {
    ENTERED_HANDLER.with(|e| {
        *e.borrow_mut() = entered;
    });
}

pub fn fizzle_trace_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed)
}

pub fn fizzle_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Relaxed)
}

/// ================================================================================================
///                                       PUBLIC FUNCTIONS
/// ================================================================================================

/// A global singleton storing fizzle data and state.
///
pub struct FizzleCell {
    inner: UnsafeCell<(bool, MaybeUninit<FizzleState>)>,
}

unsafe impl Send for FizzleCell {}
unsafe impl Sync for FizzleCell {}

impl FizzleCell {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new((false, MaybeUninit::uninit())),
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
    pub fn get(&self) -> FizzleGuard<'_> {
        let (is_init, inner) = unsafe { &mut *self.inner.get() };
        if !*is_init {
            fizzle_state_initialize(inner, is_init);
        }

        FizzleGuard { cell: self }
    }
}

pub struct FizzleGuard<'a> {
    cell: &'a FizzleCell,
}

unsafe impl Send for FizzleGuard<'_> {}
unsafe impl Sync for FizzleGuard<'_> {}

impl Deref for FizzleGuard<'_> {
    type Target = FizzleState;

    fn deref(&self) -> &Self::Target {
        unsafe { (*self.cell.inner.get()).1.assume_init_ref() }
    }
}

impl DerefMut for FizzleGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { (*self.cell.inner.get()).1.assume_init_mut() }
    }
}

// Labelling this as cold and not inlining ensures the `None` branch will be marked cold
#[cold]
#[inline(never)]
fn fizzle_state_initialize(state: &mut MaybeUninit<FizzleState>, is_init: &mut bool) {
    *state = MaybeUninit::new(FizzleState::new());
    *is_init = true;
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
        assert!(
            thread_idx == 0,
            "unexpected ThreadId value `{}` on fizzle startup",
            thread_idx
        );
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
            self.pause_current_thread();
        } else if let Some(next_process_id) = self.global().next_ready_process() {
            // ...if all threads have finished execution, move to next process.

            self.global().waking_thread_id = Some(next_process_id.thread);
            self.global.process_wake(next_process_id.process);

            // Wait for a process to delegate back to this one.
            self.pause_current_process();

            let Some(thread_id) = self.global().waking_thread_id.take() else {
                crate::abort("internal fizzle error--no waking_thread_id assigned");
            };

            if thread::current().id() != thread_id {
                self.get_thread_lock(&thread_id).post();
                self.pause_current_thread();
            }
        } else {
            // If no ready processes are left, notify the fuzzing engine
            self.notify_complete()
        };

        // Current thread isready to execute
    }

    /// Notifies the fuzzing engine that the current round of fuzzing has finished.
    /// Note that
    fn notify_complete(&mut self) {
        // Communicate that process is finished running

        // Wait for input from the fuzzing engine...

        // Mark appropriate processes/threads as ready to receive input

        // If the current running thread isn't ready to receive input, pass on to the next thread.
        if false {
            self.yield_thread(); // This won't recurse as long as new inputs are received.
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
        mem::swap(
            &mut sem,
            &mut self.thread_locks[index_of_thread(&thread_id)],
        );
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

    pub fn pause_current_thread(&mut self) {
        self.get_thread_lock(&thread::current().id()).wait()
    }

    pub fn pause_current_process(&mut self) {
        let current_process = self.local().process_id;
        self.global.process_wait(current_process);
    }
}

// File/socket objects should be global, but file descriptors should be proc-local.
//
// GlobalState should have a file descriptor map area available for a forked/exec'd process to pass its non-CLOEXEC
// fds to a child.

// We do not currently support sem_init() with pshared enabled--that would require tracking shared memory
// across processes. While this is possible, it would be a difficult and bug-ridden path to take.
// In a similar vein, we will not

// We will, however, support named process-shared semaphores.

/// State local to the current process.
pub struct ProcessState {
    pub process_id: ProcessId,
    pub fds: ValueIndex<DescriptorId, FdInfo, FIZZLE_MAX_FDS>,
    pub dirs: ValueIndex<DirectoryId, FilePath, FIZZLE_MAX_DIRS>,
    pub barriers: HashMap<BarrierPtr, BarrierInfo, FxBuildHasher>,
    pub condvars: HashMap<CondVarPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub named_semaphores: HashMap<SemaphorePtr, SemaphoreId>,
    /// Files specifically designated as being emulated.
    pub file_objs: HashMap<FilePtr, DescriptorId, FxBuildHasher>,
    pub passthrough_file_objs: HashMap<FilePtr, DescriptorId>,
    pub mutexes: HashMap<MutexPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub rwlocks: HashMap<RwLockPtr, RwLockInfo, FxBuildHasher>,
    pub semaphores: HashMap<SemaphorePtr, SemaphoreInfo>,
    pub spinlocks: HashMap<SpinlockPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub pthreads: HashMap<libc::pthread_t, ThreadId, FxBuildHasher>,
    pub ready_threads: VecDeque<ThreadId>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath,
}

impl ProcessState {
    fn new(process_id: ProcessId) -> Self {
        let debug_enabled = matches!(env::var("FIZZLE_DEBUG"), Ok(s) if s.as_str() == "1");
        let trace_enabled = matches!(env::var("FIZZLE_TRACE"), Ok(s) if s.as_str() == "1");

        DEBUG_ENABLED.store(debug_enabled, Ordering::Release);
        TRACE_ENABLED.store(trace_enabled, Ordering::Release);

        let working_directory_bytes = std::env::current_dir()
            .map(|dir| FilePath::from_raw_bytes(dir.as_os_str().as_bytes()).unwrap_or_default());
        let working_directory = working_directory_bytes.unwrap_or_default();

        Self {
            process_id, // TODO: increment each time new process is made
            fds: Default::default(),
            dirs: Default::default(),
            barriers: HashMap::with_hasher(Default::default()),
            condvars: HashMap::with_hasher(Default::default()),
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
            working_directory,
            awaiting_thread_death: HashMap::with_hasher(Default::default()),
        }
    }

    pub fn process_id(&self) -> ProcessId {
        self.process_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId {
    identifier: usize,
}

impl ProcessId {
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<ProcessId> for usize {
    fn from(val: ProcessId) -> Self {
        val.identifier
    }
}

/// An identifier used to represent a valid file descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DescriptorId {
    identifier: usize,
}

impl DescriptorId {
    pub fn new(fd: RawFd) -> Self {
        Self {
            identifier: fd as usize,
        }
    }
}

impl From<usize> for DescriptorId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<DescriptorId> for usize {
    fn from(val: DescriptorId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    identifier: usize,
}

impl FileId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for FileId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<FileId> for usize {
    fn from(val: FileId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryId {
    identifier: usize,
}

impl DirectoryId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for DirectoryId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<DirectoryId> for usize {
    fn from(val: DirectoryId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipeId {
    identifier: usize,
}

impl PipeId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for PipeId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<PipeId> for usize {
    fn from(val: PipeId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SocketId {
    identifier: usize,
}

impl SocketId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for SocketId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<SocketId> for usize {
    fn from(val: SocketId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemaphoreId {
    identifier: usize,
}

impl SemaphoreId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for SemaphoreId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<SemaphoreId> for usize {
    fn from(val: SemaphoreId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId {
    identifier: usize,
}

impl BufferId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for BufferId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<BufferId> for usize {
    fn from(val: BufferId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FifoId {
    identifier: usize,
}

impl FifoId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<FifoId> for usize {
    fn from(val: FifoId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MessageQueueId {
    identifier: usize,
}

impl MessageQueueId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for MessageQueueId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<MessageQueueId> for usize {
    fn from(val: MessageQueueId) -> Self {
        val.identifier
    }
}

/// State/data shared among all processes in a fizzle execution.
pub struct InterprocessState {
    /// The next process ID available to be assigned to a new process.
    next_process_id: ProcessId,
    /// The thread identifier to be executed by the waking process.
    waking_thread_id: Option<ThreadId>,
    ready_workers: Queue<WorkerId, FIZZLE_MAX_READY_PROCESSES>,
    pub file_paths: FnvIndexMap<FilePath, FileId, FIZZLE_MAX_FILE_PATHS>,
    pub files: ValueIndex<FileId, FileInfo, FIZZLE_MAX_FILES>,
    pub sem_paths: FnvIndexMap<SemPath, SemaphoreId, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub semaphores: ValueIndex<SemaphoreId, SemaphoreInfo, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub pipes: ValueIndex<PipeId, PipeInfo, FIZZLE_MAX_PIPES>,
    pub message_queues: ValueIndex<MessageQueueId, MessageQueueInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
    pub sockets: ValueIndex<SocketId, SocketInfo, FIZZLE_MAX_SOCKETS>,
    pub buffers: ValueIndex<BufferId, RingBuffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub transfer_fds: Option<ValueIndex<DescriptorId, FdInfo, FIZZLE_MAX_FDS>>,
}

impl InterprocessState {
    fn new() -> Self {
        Self {
            next_process_id: ProcessId::new(1), // First process takes 0, so next is 1
            waking_thread_id: None,
            ready_workers: Queue::new(),
            file_paths: FnvIndexMap::new(),
            files: Default::default(),
            sem_paths: FnvIndexMap::new(),
            semaphores: Default::default(),
            pipes: Default::default(),
            message_queues: Default::default(),
            sockets: Default::default(),
            buffers: Default::default(),
            transfer_fds: None,
        }
    }

    /// Assigns the next available process ID and increments it internally.
    pub fn assign_process_id(&mut self) -> ProcessId {
        let process_id = self.next_process_id;
        self.next_process_id.identifier += 1;
        process_id
    }

    /// Retrieves the next available process/thread pair that has work to execute.
    pub fn next_ready_process(&mut self) -> Option<WorkerId> {
        let worker_id = self.ready_workers.dequeue()?;
        Some(worker_id)
    }

    /// Marks the given process/thread pair as having further work to execute.
    pub fn mark_worker_ready(&mut self, worker_id: WorkerId) {
        self.ready_workers.enqueue(worker_id).unwrap();
    }
}

/// A hasher that correctly outputs the internal value of a [`ThreadId`] for its hash.
pub struct ThreadHasher {
    value: u64,
}

impl ThreadHasher {
    pub fn new() -> Self {
        Self { value: 0 }
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
        self.value += i;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BarrierPtr(usize);

impl From<*mut libc::pthread_barrier_t> for BarrierPtr {
    fn from(value: *mut libc::pthread_barrier_t) -> Self {
        BarrierPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CondVarPtr(usize);

impl From<*mut libc::pthread_cond_t> for CondVarPtr {
    fn from(value: *mut libc::pthread_cond_t) -> Self {
        CondVarPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutexPtr(usize);

impl From<*mut libc::pthread_mutex_t> for MutexPtr {
    fn from(value: *mut libc::pthread_mutex_t) -> Self {
        MutexPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RwLockPtr(usize);

impl From<*mut libc::pthread_rwlock_t> for RwLockPtr {
    fn from(value: *mut libc::pthread_rwlock_t) -> Self {
        RwLockPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpinlockPtr(usize);

impl From<*mut libc::pthread_spinlock_t> for SpinlockPtr {
    fn from(value: *mut libc::pthread_spinlock_t) -> Self {
        SpinlockPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemaphorePtr(usize);

impl From<*mut libc::sem_t> for SemaphorePtr {
    fn from(value: *mut libc::sem_t) -> Self {
        SemaphorePtr(value as usize)
    }
}

impl SemaphorePtr {
    pub(crate) fn to_mut_ptr(self) -> *mut libc::sem_t {
        self.0 as *mut libc::sem_t
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePtr(usize);

impl From<*mut libc::FILE> for FilePtr {
    fn from(value: *mut libc::FILE) -> Self {
        FilePtr(value as usize)
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

#[derive(Debug)]
pub struct PipeInfo {
    /// The transmission mode of the packet.
    ///
    /// See [`PipeMode`] for more details.
    pub mode: PipeMode,
    /// The peer pipe that this pipe is connected to.
    ///
    /// If this value is `None`, then the pipe has broken (e.g., the other end has shut).
    pub peer: Option<PipeId>,
    /// The buffer this pipe reads in data from.
    pub read_buf: BufferId,
}

/// The mode of operation by which data is passed over the pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeMode {
    /// Performs I/O in "packet" mode--writes are treated as individual packets.
    Direct,
    /// Performs I/O as if data is a constant stream.
    Streamed,
}

#[derive(Debug)]
pub struct SocketInfo {}

#[derive(Debug)]
pub struct MessageQueueInfo {}

impl FileInfo {
    /// Creates a new temporary file.
    pub fn new() -> Self {
        Self { temporary: true }
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
    pub refs: usize,
    pub unlinked: bool,
    pub value: usize,
    pub waiting: Deque<WorkerId, FIZZLE_MAX_WAITING_SEMAPHORES>,
}

/// The unique identifying information for a given thread in a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerId {
    pub process: ProcessId,
    pub thread: ThreadId,
}

// ---=== Helper Functions ===---

fn index_of_thread(thread: &ThreadId) -> usize {
    let mut hasher = ThreadHasher::new();
    thread.hash(&mut hasher);
    hasher.finish() as usize
}
