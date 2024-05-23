mod comptime;
pub mod fd;
pub mod identifiers;
pub mod plugins;

use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString};
use std::hash::{Hash, Hasher};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::{array, env, mem, ptr, thread};

use fizzle_common::io::SocketLocation;
use fizzle_common::path::{FilePath, SemPath};
use fizzle_common::storage::{RingBuffer, ValueIndex};

use heapless::spsc::Queue;

use fxhash::FxBuildHasher;
use heapless::{Deque, FnvIndexMap};

use crate::constants::*;
use crate::semaphore::Semaphore;
use crate::state::plugins::PluginConfig;

use self::identifiers::*;
use self::fd::FdInfo;
use self::plugins::{PluginId, PluginMappings, PluginModules};

// TODO: we will assume that the main process cannot exit. This should be documented.
// Likewise, there should exist an env variable (like `FIZZLE_NOEXIT=status_code`) that, when set,
// ensures that the main process does not exit when passed the given status code.

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = const { RefCell::new(false) };
}

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

/// Marks the thread as currently executing within a fizzle handler.
pub fn set_entered_handler(entered: bool) {
    ENTERED_HANDLER.with(|e| {
        *e.borrow_mut() = entered;
    });
}

static STRICT_MODE: AtomicBool = AtomicBool::new(false);

pub fn strict_mode() -> bool {
    STRICT_MODE.load(Ordering::Relaxed)
}

/// Retrieves the current fizzle state.
///
/// SAFETY: this function MUST ONLY be called within `ld_preload.rs` or `dyld_insert_libraries.rs`.
/// Accessing the global `FizzleState` variable multiple times in one scope will lead to UB.
pub static FIZZLE_STATE: FizzleCell = FizzleCell::new();

/// A global singleton storing fizzle data and state.
pub struct FizzleCell {
    inner: UnsafeCell<(bool, MaybeUninit<FizzleContext>)>,
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
    type Target = FizzleContext;

    fn deref(&self) -> &Self::Target {
        unsafe { (*self.cell.inner.get()).1.assume_init_ref() }
    }
}

impl DerefMut for FizzleGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { (*self.cell.inner.get()).1.assume_init_mut() }
    }
}

/// This is marked as `cold` and kept un-inlined to ensure that it will not lead to frequent branch
/// misses (other than the first time when it should be called).
#[cold]
#[inline(never)]
fn fizzle_state_initialize(state: &mut MaybeUninit<FizzleContext>, is_init: &mut bool) {
    env_logger::init();
    log::trace!("env_logger initialized.");
    log::trace!("Running fizzle state initialization");

    *state = MaybeUninit::new(FizzleContext::new());
    *is_init = true;

    log::trace!("Fizzle state initialization complete");
}

/// All state that fizzle functions receive access to.
pub struct FizzleContext {
    thread_locks: [Option<Semaphore>; FIZZLE_MAX_THREADS],
    /// `local`, as in local to the current executing process.
    process_state: Box<ProcessState>,
    /// `global`, as in shared across all processes in a given fizzle harness.
    shared_memory: *mut libc::c_void,
    // shmem_fd: RawFd,
}

impl FizzleContext {
    const SHMEM_LENGTH: usize = (mem::size_of::<libc::sem_t>() * FIZZLE_MAX_PROCESSES)
        + mem::size_of::<InterprocessState>();

    fn create_thread_locks() -> [Option<Semaphore>; 256] {
        let thread_idx = index_of_thread(&thread::current().id());
        assert!(
            thread_idx == 1,
            "unexpected ThreadId value `{}` on fizzle startup",
            thread_idx
        );

        // Initialize the current thread's lock
        array::from_fn(|i| if i == thread_idx { Some(Semaphore::new(0)) } else { None })
    }

    unsafe fn interprocess_state(shared_memory: *mut libc::c_void) -> *mut InterprocessState {
        (shared_memory as *mut libc::sem_t).add(FIZZLE_MAX_PROCESSES)
                as *mut InterprocessState
    }

    unsafe fn open_shmem(shmem_location: *const libc::c_char, create_shmem: bool) -> *mut libc::c_void {
        let mode = libc::S_IRUSR | libc::S_IWUSR;
        let oflag = libc::O_RDWR | match create_shmem {
            true => libc::O_CREAT | libc::O_EXCL,
            false => 0,
        };

        let fd = unsafe { libc::shm_open(shmem_location, oflag, mode) };
        if fd < 0 {
            panic!("couldn't open fizzle global shared memory")
        }

        let shared_memory = unsafe {
            libc::mmap(
                ptr::null_mut(),
                Self::SHMEM_LENGTH,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if shared_memory == libc::MAP_FAILED {
            panic!("unable to memory-map fizzle global shared memory")
        }

        if unsafe { libc::ftruncate(fd, Self::SHMEM_LENGTH as libc::off_t) } != 0 {
            panic!("unable to ftruncate fizzle global shared memory (potential out-of-memory condition)");
        }

        if unsafe { libc::close(fd) } != 0 {
            panic!("failed to clean up fizzle global shared memory fd")
        }

        // TODO: when will this shared memory ever be unlinked? Dangling resource...
        // let ret = unsafe { libc::shm_unlink(name.as_ptr()) }; (this shouldn't be here...)
        // Maybe we leave the fd open but close the shmem... that way it all frees up when the process dies

        shared_memory
    }

    fn create_shmem_label() -> CString {
        let mut name = Vec::from([0u8; 64]);
        if unsafe { libc::getrandom(name.as_mut_ptr() as *mut libc::c_void, name.len(), 0)} != name.len() as isize {
            panic!("fizzle shared memory initialization failed due to getrandom")
        }

        for c in name.iter_mut() {
            // Encode random characters to be [0-9?@A-Za-Z] (64 options)
            *c /= 4; // reduce options to 0..=63
            *c += 48;

            if *c >= 58 {
                *c += 5;
            }

            if *c >= 91 {
                *c += 6;
            }
        }

        name[..15].copy_from_slice(b"/fizzle_shared_");

        name.push(0u8);
        unsafe { CString::from_vec_unchecked(name) }
    }

    unsafe fn initialize_shmem_contents(shared_memory: *mut libc::c_void) {
        let mut sem_ptr = shared_memory as *mut libc::sem_t;
        for _ in 0..FIZZLE_MAX_PROCESSES {
            if libc::sem_init(sem_ptr, libc::PTHREAD_PROCESS_SHARED, 1) != 0 {
                panic!("unable to initialize per-process semaphores for IpcMemory");
            }

            sem_ptr = sem_ptr.add(1);
        }

        InterprocessState::initialize(
            sem_ptr as *mut MaybeUninit<InterprocessState>,
        );
    }

    /// Initializes the fizzle state.
    ///
    /// This will be called when the first shimmed libc call is executed--only one thread
    /// is executing at the time, and no libc calls have completed yet.
    fn new() -> Self {
        let process_id: ProcessId;
        let shared_memory: *mut libc::c_void;
        let plugins: Option<PluginModules>;

        log::info!("initializing global shared memory");

        let shmem_label = unsafe { libc::getenv(FIZZLE_MEMORY_ENV.as_ptr()) };
        if !shmem_label.is_null() {
            // This is a child process of a fizzle fuzzing process--don't initialize the shared memory.
            log::info!(
                "FIZZLE_MEMORY env variable detected--opening global shared memory..."
            );
            plugins = None;

            unsafe {
                shared_memory = Self::open_shmem(shmem_label, false);
                process_id = (*Self::interprocess_state(shared_memory)).assign_process_id();
            }

        } else {
            process_id = ProcessId::new(1);

            let mut plugin_config = PluginConfig::new();
            comptime::populate_plugins(&mut plugin_config);
            plugins = Some(plugin_config.modules);

            let shmem_label = Self::create_shmem_label();
            
            unsafe {
                shared_memory = Self::open_shmem(shmem_label.as_ptr(), true);
                Self::initialize_shmem_contents(shared_memory);
                (*Self::interprocess_state(shared_memory)).load_plugin_mappings(plugin_config.mappings);
            }
        }

        Self {
            thread_locks: Self::create_thread_locks(),
            process_state: Box::new(ProcessState::new(process_id, plugins)),
            shared_memory,
        }
    }

    /// Retrieve state specific to the process.
    pub fn local(&mut self) -> &mut ProcessState {
        &mut self.process_state
    }

    /// Retrieve state shared across processes.
    pub fn global(&mut self) -> &mut InterprocessState {
        unsafe { &mut (*Self::interprocess_state(self.shared_memory)) }
    }

    /// Pauses execution of the current thread and delegates control flow to another thread/process.
    /// Once all threads/processes have finished executing, this returns control flow to the primary
    /// fuzzing process, which signals to the fuzzer that it is ready for the next input.
    pub fn yield_thread(&mut self) {
        // Check to see if all threads have finished execution for this process
        if let Some(thread_id) = self.process_state.ready_threads.pop_front() {
            // ...if not, then run the next one.
            self.get_thread_lock(&thread_id).post();
            self.pause_current_thread();
        } else if let Some(next_process_id) = self.global().next_ready_process() {
            // ...if all threads have finished execution, move to next process.

            self.global().waking_thread_id = Some(next_process_id.thread);
            self.wake_process(next_process_id.process);

            // Wait for a process to delegate back to this one.
            self.pause_current_process();

            let Some(thread_id) = self.global().waking_thread_id.take() else {
                panic!("internal fizzle error--no waking_thread_id assigned");
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
        self.process_state.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = self.process_state.awaiting_thread_death.remove(&thread_id) {
            for awaiting_id in awaiting_threads {
                self.process_state.ready_threads.push_back(awaiting_id);
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

    pub fn pause_current_thread(&mut self) {
        self.get_thread_lock(&thread::current().id()).wait()
    }

    /// Retrieves the semaphore associated with the thread idendified by `thread_id`.
    fn get_thread_lock(&self, thread_id: &ThreadId) -> &Semaphore {
        let thread_idx = index_of_thread(thread_id);
        assert!(thread_idx < self.thread_locks.len(), "too many threads spawned during fizzle execution (ThreadID out of range)--increase FIZZLE_MAX_THREADS constant during fizzle compilation");
        self.thread_locks[thread_idx].as_ref().unwrap()
    }

    pub fn pause_current_process(&mut self) {
        let process_idx: usize = self.local().process_id.into();
        if process_idx >= FIZZLE_MAX_PROCESSES {
            panic!("too many processes spawned for fizzle (`pause_current_process()` fatal error)")
        }

        unsafe {
            let process_semaphore = (self.shared_memory as *mut libc::sem_t).add(process_idx);
            while libc::sem_wait(process_semaphore) != 0 {}
        }
    }

    fn wake_process(&mut self, process_id: ProcessId) {
        let process_idx: usize = process_id.into();
        if process_idx >= FIZZLE_MAX_PROCESSES {
            panic!("too many processes spawned for fizzle (`wake_process()` fatal error)")
        }

        unsafe {
            let process_semaphore = (self.shared_memory as *mut libc::sem_t).add(process_idx);
            if libc::sem_post(process_semaphore) != 0 {
                panic!("fizzle internal error--unable to wake process with `sem_post()`")
            }
        }
    }
}

// We do not currently support sem_init() with pshared enabled--that would require tracking shared memory
// across processes. While this is possible, it would be a difficult and bug-ridden path to take.
// In a similar vein, we will not

// We will, however, support named process-shared semaphores.

/// State local to the current process.
pub struct ProcessState {
    pub process_id: ProcessId,
    /// Plugin modules for handling I/O.
    ///
    /// This field is only `Some` in the parent process; all other processes must delegate control
    /// flow to it in order to handle plugin I/O.
    pub plugin_modules: Option<PluginModules>,
    pub fds: ValueIndex<DescriptorId, FdInfo, FIZZLE_MAX_FDS>,
    pub dirs: ValueIndex<DirectoryId, FilePath, FIZZLE_MAX_DIRS>,
    pub barriers: HashMap<BarrierPtr, BarrierInfo, FxBuildHasher>,
    pub condvars: HashMap<CondVarPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub named_semaphores: HashMap<SemaphorePtr, SemaphoreId>,
    /// Files specifically designated as being emulated.
    pub file_objs: HashMap<FilePtr, FileObject, FxBuildHasher>,
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
    fn new(
        process_id: ProcessId,
        plugin_modules: Option<PluginModules>,
    ) -> Self {
        let strict_mode = matches!(env::var(FIZZLE_STRICT_ENV), Ok(s) if s.as_str() == "1");
        STRICT_MODE.store(strict_mode, Ordering::Release);

        let mut working_dir = [0u8; 256];
        let cwd = unsafe { libc::getcwd(working_dir.as_mut_ptr() as *mut libc::c_char, 255) };
        if cwd.is_null() {
            panic!("fizzle missing working directory on startup");
        }
        let working_directory = FilePath::from_cstr(unsafe { CStr::from_ptr(cwd) }).unwrap();

        Self {
            process_id, // TODO: increment each time new process is made
            plugin_modules,
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
    pub socket_locations: FnvIndexMap<SocketLocation, SocketId, FIZZLE_MAX_SOCKETS>,
    pub sockets: ValueIndex<SocketId, SocketInfo, FIZZLE_MAX_SOCKETS>,
    pub buffers: ValueIndex<BufferId, RingBuffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub transfer_fds: Option<ValueIndex<DescriptorId, FdInfo, FIZZLE_MAX_FDS>>,
}

impl InterprocessState {
    // TODO: initialize() is unsafe--whenever we change the fields in InterprocessState, it becomes
    // unsound until we add the corresponding definition. We should really change it to a trait +
    // proc macro derive.
    /// Takes an uninitialized InterprocessState and initializes it in place.
    unsafe fn initialize(
        state: *mut MaybeUninit<InterprocessState>,
    ) {
        let state = state as *mut InterprocessState;
        *ptr::addr_of_mut!((*state).next_process_id) = ProcessId::new(1);
        *ptr::addr_of_mut!((*state).waking_thread_id) = None;
        *ptr::addr_of_mut!((*state).ready_workers) = Queue::new();
        *ptr::addr_of_mut!((*state).file_paths) = FnvIndexMap::new();
        ValueIndex::initialize(ptr::addr_of_mut!((*state).files)
            as *mut MaybeUninit<ValueIndex<FileId, FileInfo, FIZZLE_MAX_FILES>>);
        *ptr::addr_of_mut!((*state).sem_paths) = FnvIndexMap::new();
        ValueIndex::initialize(ptr::addr_of_mut!((*state).semaphores)
            as *mut MaybeUninit<
                ValueIndex<SemaphoreId, SemaphoreInfo, FIZZLE_MAX_NAMED_SEMAPHORES>,
            >);
        ValueIndex::initialize(ptr::addr_of_mut!((*state).pipes)
            as *mut MaybeUninit<ValueIndex<PipeId, PipeInfo, FIZZLE_MAX_PIPES>>);
        ValueIndex::initialize(ptr::addr_of_mut!((*state).message_queues)
            as *mut MaybeUninit<
                ValueIndex<MessageQueueId, MessageQueueInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
            >);
        *ptr::addr_of_mut!((*state).socket_locations) = FnvIndexMap::new();
        ValueIndex::initialize(ptr::addr_of_mut!((*state).sockets)
            as *mut MaybeUninit<ValueIndex<SocketId, SocketInfo, FIZZLE_MAX_SOCKETS>>);
        ValueIndex::initialize(ptr::addr_of_mut!((*state).buffers)
            as *mut MaybeUninit<
                ValueIndex<BufferId, RingBuffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
            >);
        *ptr::addr_of_mut!((*state).transfer_fds) = None;
    }

    fn load_plugin_mappings(&mut self, plugin_mappings: PluginMappings) {

    }

    /// Assigns the next available process ID and increments it internally.
    pub fn assign_process_id(&mut self) -> ProcessId {
        let process_id = self.next_process_id;
        self.next_process_id = ProcessId::new(usize::from(process_id) + 1);
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

    /// This method returns `Ok` if the file was created, and `Err` if a file already
    /// exists at the given path.
    pub fn create_file(&mut self, path: FilePath) -> Result<FileId, FileId> {
        match self.file_paths.get(&path) {
            Some(&id) => Err(id),
            None => {
                let buffer_id = self.buffers.put(RingBuffer::new());
                let file_id = self.files.put(FileInfo::new(buffer_id));
                Ok(file_id)
            }
        }
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

pub struct FileObject {
    pub descriptor_id: DescriptorId,
    pub buf: RingBuffer<FIZZLE_FOPEN_BUFSIZE>,
}

#[derive(Debug)]
pub struct BarrierInfo {
    pub curr: Vec<ThreadId>,
    pub needed: usize,
}

#[derive(Debug)]
pub struct FileInfo {
    pub backend: FileBackend,
}

#[derive(Debug)]
pub enum FileBackend {
    /// Storage is modeled using a RingBuffer (issues with this--consider fixing...)
    Emulated(BufferId), 
    Plugin(PluginId),
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
    pub fn new(buffer_id: BufferId) -> Self {
        Self {
            backend: FileBackend::Emulated(buffer_id)
        }
    }
}

impl From<PluginId> for FileInfo {
    fn from(plugin_id: PluginId) -> Self {
        Self {
            backend: FileBackend::Plugin(plugin_id),
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
    pub refs: usize,
    pub unlinked: bool,
    pub value: usize,
    pub waiting: Deque<WorkerId, FIZZLE_MAX_WAITING_SEMAPHORES>,
}

// ---=== Helper Functions ===---

fn index_of_thread(thread: &ThreadId) -> usize {
    let mut hasher = ThreadHasher::new();
    thread.hash(&mut hasher);
    hasher.finish() as usize
}
