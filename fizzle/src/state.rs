pub mod backend;
pub mod comptime;
pub mod fd;
pub mod identifiers;
pub mod plugins;

use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, CString};
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::{Deref, DerefMut};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::{array, env, mem, ptr, thread};

use fizzle_common::io::{AddressFamily, TransportAddress, TransportProtocol};
use fizzle_common::path::{FilePath, SemPath};
use fizzle_common::storage::{RingBuffer, ValueIndex};

use fizzle_plugin::{IoEndpointVariant, StreamId};
use heapless::spsc::Queue;

use fxhash::FxBuildHasher;
use heapless::{Deque, FnvIndexMap};

use crate::constants::*;
use crate::semaphore::Semaphore;
use crate::state::plugins::PluginConfig;

use self::backend::{ConnectedBackend, ConnectingBackend, ConnectionlessBackend, FileBackend, PendingBackend, ServerBackend, StandardFeedback, StandardPlugin, StdioBackend};
use self::fd::FdInfo;
use self::identifiers::*;
use self::plugins::{IoEmulationType, PluginConfigEndpoint, PluginModules};

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

#[no_mangle]
extern "C" fn fizzle_atexit_suspend() {
    loop {
        loop {
            // TODO: clean up any dangling polling items here, like for `_exit()`/`exit()`
            FIZZLE_STATE.get().yield_thread()
        }
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

    unsafe {
        if state.assume_init_mut().local().suspend_on_exit {
            libc::atexit(fizzle_atexit_suspend); // Registered before any other atexit handler...
        }
    }

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
        array::from_fn(|i| {
            if i == thread_idx {
                Some(Semaphore::new(0))
            } else {
                None
            }
        })
    }

    unsafe fn interprocess_state(shared_memory: *mut libc::c_void) -> *mut InterprocessState {
        (shared_memory as *mut libc::sem_t).add(FIZZLE_MAX_PROCESSES) as *mut InterprocessState
    }

    unsafe fn open_shmem(
        shmem_location: *const libc::c_char,
        create_shmem: bool,
    ) -> *mut libc::c_void {
        let mode = libc::S_IRUSR | libc::S_IWUSR;
        let oflag = libc::O_RDWR
            | match create_shmem {
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
        if unsafe { libc::getrandom(name.as_mut_ptr() as *mut libc::c_void, name.len(), 0) }
            != name.len() as isize
        {
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

        InterprocessState::initialize(sem_ptr as *mut InterprocessState);
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
            log::info!("FIZZLE_MEMORY env variable detected--opening global shared memory...");
            plugins = None;

            unsafe {
                shared_memory = Self::open_shmem(shmem_label, false);
                process_id = (*Self::interprocess_state(shared_memory)).assign_process_id();
            }
        } else {
            process_id = ProcessId::from(0);

            let mut plugin_config = PluginConfig::new();
            comptime::populate_plugins(&mut plugin_config);
            plugins = Some(plugin_config.modules);

            let shmem_label = Self::create_shmem_label();

            unsafe {
                shared_memory = Self::open_shmem(shmem_label.as_ptr(), true);
                Self::initialize_shmem_contents(shared_memory);
                (*Self::interprocess_state(shared_memory))
                    .load_config_mappings(plugin_config.endpoints);
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
        let mut next_worker = None;

        // pop PollerId values off `ready_pollers` one at a time
        while let Some(item) = self.global().ready.dequeue() {
            match item {
                ReadyInfo::Worker(worker_id) => {
                    // new_raised_events will be empty here
                    next_worker = Some(worker_id);
                    break;
                }
                ReadyInfo::Poller(poller_id) => {
                    let global = self.global();
                    let poller_info = global.pollers.get(poller_id).unwrap();
                    for &polled_id in poller_info.polled_events.iter() {
                        let polled_info = global.polled_events.get_mut(polled_id).unwrap();
                        if polled_info.event_raised {
                            next_worker = Some(poller_info.worker_id);
                            break
                        }
                        
                    }
                    // if current poller has all PolledId values that have false flags, move on to next
                }
            }
        }

        if let Some(worker_id) = next_worker {
            self.global().waking_thread_id = Some(worker_id.thread_id);
            // self.global().raised_events = new_raised_events;

            if worker_id.process_id != self.local().process_id {
                self.wake_process(worker_id.process_id);
                self.pause_current_process();
            }

            let Some(thread_id) = self.global().waking_thread_id.take() else {
                panic!("internal fizzle error--no waking_thread_id assigned");
            };

            if thread::current().id() != thread_id {
                self.get_thread_lock(&thread_id).post();
                self.pause_current_thread();
            }
        } else if self::plugins::run_plugins() { // Plugins have queued more workers as ready
            // This shouldn't lead to a stack overflow unless `run_plugins` erroneously
            // returns `true` but doesn't schedule new workers.
            self.yield_thread();
        
        } else {
            // No events were triggered for any pollers--move on to next input
            self.fuzz_round_complete();
        }
    }

    /// Notifies the fuzzing engine that the current round of fuzzing has finished.
    /// Note that
    fn fuzz_round_complete(&mut self) {
        // Communicate that process is finished running

        // Wait for input from the fuzzing engine...

        // Mark appropriate processes/threads as ready to receive input

        // If the current running thread isn't ready to receive input, pass on to the next thread.
        if false {
            self.yield_thread(); // This won't recurse as long as new inputs are received.
        }

        todo!()
    }

    /// Adds a thread from the current process to the `ready` queue.
    pub fn add_ready_thread(&mut self, thread_id: ThreadId) {
        let process_id = self.local().process_id;
        self.global()
            .ready
            .enqueue(ReadyInfo::Worker(WorkerId {
                process_id,
                thread_id,
            }))
            .unwrap();
    }

    /// Ceases execution of the current thread.
    pub fn exit_thread(&mut self, ret: *mut libc::c_void) -> ! {
        let thread_id = thread::current().id();

        // Mark this thread as dead
        self.process_state.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = self.process_state.awaiting_thread_death.remove(&thread_id)
        {
            for thread_id in awaiting_threads {
                let process_id = self.local().process_id;
                self.global()
                    .ready
                    .enqueue(ReadyInfo::Worker(WorkerId {
                        process_id,
                        thread_id,
                    }))
                    .unwrap();
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

    // call this whenever waiting for a single poll event
    pub fn poll_until_ready(&mut self, polled_id: PolledId) {
        if !self.polled_is_ready(polled_id) {
            let poller_id = self.new_poller();
            self.register_poller(poller_id, polled_id);
            self.yield_thread();
            self.delete_poller(poller_id);
        }
        // Set polled.poller_dispatched = false;
    }

    pub fn polled_is_ready(&mut self, polled_id: PolledId) -> bool {
        let polled = self.global().polled_events.get(polled_id).unwrap();
        polled.event_raised
    }

    // call this whenever new data comes into a buffer
    pub fn raise_polled(&mut self, polled_id: PolledId) {
        let polled = self.global().polled_events.get(polled_id).unwrap();
        if !polled.event_raised {
            let pollers = polled.pollers.clone();
            for poller in pollers {
                if !self.global().pollers.get(poller).unwrap().in_raised_queue {
                    self.global().ready.enqueue(ReadyInfo::Poller(poller)).unwrap();
                }
            }
        }
    }

    // if buffer is empty, then call this
    pub fn lower_polled(&mut self, polled_id: PolledId) {
        self.global()
            .polled_events
            .get_mut(polled_id)
            .unwrap()
            .event_raised = false;
    }

    /*
    // else, call this
    pub fn enqueue_next_polled(&mut self, polled_id: PolledId) {
        let polled = self.global().polled_events.get_mut(polled_id).unwrap();
        if let Some(poller_id) = polled.pollers.dequeue() {
            polled.poller_dispatched = true;
            self.global()
                .ready
                .enqueue(ReadyInfo::Poller(poller_id))
                .unwrap();
        }
    }
    */

    /// Creates a new poller for the currently executing worker.
    pub fn new_poller(&mut self) -> PollerId {
        let worker_id = self.current_worker_id();

        self.global().pollers.put(PollerInfo {
            worker_id,
            polled_events: heapless::Vec::new(),
            in_raised_queue: false,
        })
    }

    /// Registers `poller_id` as waiting on `polled_id`.
    pub fn register_poller(&mut self, poller_id: PollerId, polled_id: PolledId) {
        let poller = self.global().pollers.get_mut(poller_id).unwrap();
        poller.polled_events.push(polled_id).unwrap();
        let polled = self.global().polled_events.get_mut(polled_id).unwrap();
        polled.pollers.push(poller_id).unwrap();
    }

    // Ugh. This looks like O(n^2)...
    /// Deletes the given poller, removing any references to it from `Polled` objects.
    pub fn delete_poller(&mut self, poller_id: PollerId) {
        let poller = self.global().pollers.remove(poller_id).unwrap();
        if poller.in_raised_queue {
            // TODO: remove poller from raised queue...
        }
    }

    pub fn current_worker_id(&mut self) -> WorkerId {
        WorkerId {
            process_id: self.local().process_id(),
            thread_id: thread::current().id(),
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



// Challenge: map a given endpoint to a (PluginId, IoEndpoint, StreamId) tuple with buffers + Polled instances for I/O

// We do not currently support sem_init() with pshared enabled--that would require tracking shared memory
// across processes. While this is possible, it would be a difficult and bug-ridden path to take.
// In a similar vein, we will not

// We will, however, support named process-shared semaphores.

/// State local to the current process.
pub struct ProcessState {
    pub process_id: ProcessId,
    pub suspend_on_exit: bool,
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
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath,
}

impl ProcessState {
    fn new(process_id: ProcessId, plugin_modules: Option<PluginModules>) -> Self {
        let strict_mode = matches!(env::var(FIZZLE_STRICT_ENV), Ok(s) if s.as_str() == "1");
        let suspend_on_exit = matches!(env::var(FIZZLE_NOEXIT_ENV), Ok(s) if s.as_str() == "1");
        STRICT_MODE.store(strict_mode, Ordering::Release);

        let mut working_dir = [0u8; 256];
        let cwd = unsafe { libc::getcwd(working_dir.as_mut_ptr() as *mut libc::c_char, 255) };
        if cwd.is_null() {
            panic!("fizzle missing working directory on startup");
        }
        let working_directory = FilePath::from_cstr(unsafe { CStr::from_ptr(cwd) }).unwrap();

        Self {
            process_id, // TODO: increment each time new process is made
            suspend_on_exit,
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
            terminated_threads: HashSet::with_hasher(Default::default()),
            working_directory,
            awaiting_thread_death: HashMap::with_hasher(Default::default()),
        }
    }

    pub fn process_id(&self) -> ProcessId {
        self.process_id
    }
}

pub struct FileObject {
    pub descriptor_id: DescriptorId,
    pub buf: RingBuffer<FIZZLE_FOPEN_BUFSIZE>,
}

// Each time a Polled is *raised* (i.e., goes from `event_raised: false` to `event_raised: true`),
// the PolledInfo will move all of its `pollers` into the ready queue (if they are not already there).
#[derive(Debug)]
pub struct PolledInfo {
    /// Pollers that this Polled instance is meant to awaken
    pub pollers: heapless::Vec<PollerId, FIZZLE_MAX_PER_EVENT_QUEUED_POLLERS>,
    /// Indicates that the item being polled is "ready" for the `Poller`.
    pub event_raised: bool,
    // /// Indicates that a `Poller` has been sent to the ready queue from this `Polled` instance and
    // /// has not yet been executed.
    // pub poller_dispatched: bool,
}

impl PolledInfo {
    pub fn new() -> Self {
        Self {
            pollers: heapless::Vec::new(),
            event_raised: false,
        }
    }

    pub fn new_raised() -> Self {
        Self {
            pollers: heapless::Vec::new(),
            event_raised: true,
        }
    }
}

/*
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolledItem {
    None,
    PendingClients,
    PluginInput(PluginId),
    PluginOutput(PluginId),
    Socket(SocketId),
}
*/

#[derive(Debug)]
pub struct SocketLocationInfo {
    /// The socket bound to the given location.
    pub bound_socket: Option<SocketId>,
    /// Points to an optional linked list of clients that are awaiting this location to exist.
    pub pending: Option<PendingInfo>,
}

#[derive(Debug)]
pub struct PendingInfo {
    pub client: SocketId,
    pub poll: PolledId,
}

#[derive(Debug)]
pub struct PollerInfo {
    worker_id: WorkerId,
    polled_events: heapless::Vec<PolledId, FIZZLE_MAX_PER_POLLER_QUEUED_EVENTS>,
    in_raised_queue: bool,
}

// INVARIANT: a Worker must only ever be awakened once it has actions to take.
// We now seek to accomplish this through the polling infrastructure in InterprocessState.

// TODO: rename...
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyInfo {
    Poller(PollerId),
    Worker(WorkerId),
}

#[derive(Debug)]
pub enum SocketState {
    Connectionless(ConnectionlessSocket),
    Unassociated(UnassociatedSocket),
    Server(ServerSocket),
    PendingConnection(PendingSocket),
    Connecting(ConnectingSocket),
    Connected(ConnectedSocket),
    Error,
}

#[derive(Debug)]
pub struct ConnectionlessSocket {
    pub backend: ConnectionlessBackend,
    pub local_addr: SocketAddr,
    pub rem_addr: Option<SocketAddr>,
}

#[derive(Debug)]
pub struct UnassociatedSocket {
    pub local_addr: Option<TransportAddress>,
    pub family: AddressFamily,
    pub protocol: TransportProtocol,
}

#[derive(Debug)]
pub struct ServerSocket {
    pub backend: ServerBackend,
    pub local_addr: TransportAddress,
    pub connecting: Queue<SocketId, FIZZLE_SOMAXCONN>,
    pub ready_to_connect: PolledId,
}

#[derive(Debug)]
pub struct PendingSocket {
    pub backend: PendingBackend,
    pub next_pending: Option<SocketId>,
    pub rem_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectingSocket {
    pub backend: ConnectingBackend,
    pub connect_polled: PolledId,
    pub local_addr: TransportAddress,
    pub rem_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectedSocket {
    pub backend: ConnectedBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: TransportAddress,
}

// Runtime active plugin I/O information
pub struct PluginInfo {
    pub endpoint: IoEndpointVariant,
    pub stream: StreamId,
    /// Information to be passed to the plugin.
    pub write_buf: BufferId,
    pub write_polled: PolledId,
    /// Information the plugin returns to the application.
    pub read_buf: BufferId,
    pub read_polled: PolledId,
    /// The plugin module to read/write from.
    pub module_id: PluginModuleId,
}

#[derive(Debug)]
pub struct EpollInfo {
    pub interests: FnvIndexMap<DescriptorId, EpollInterest, FIZZLE_MAX_EPOLL_FDS>,
}

#[derive(Clone, Copy, Debug)]
pub struct EpollInterest {
    pub direction: EpollDirection,
    pub descriptor: RawFd,
    pub user_data: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpollDirection {
    None,
    Read(PolledStatus),
    Write(PolledStatus),
    Both(PolledStatus, PolledStatus),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolledStatus {
    Pollable(PolledId),
    /// The requested object to be polled has had its peer closed.
//    Shutdown,
    /// The file descriptor was invalid.
    BadFd,
    /// The requested object will never return polled output (such as attempting to read `stdout`).
    NotPollable,
    /// The requested object will immediately return polled output (such as writing to `stderr`).
    ImmediatelyPollable,
}

pub struct InterprocessState {
    next_process_id: ProcessId,
    /// The next StreamId available to be assigned to an emulated stream.
    next_stream_id: StreamId,
    /// The next ephemeral port to be assigned to a socket.
    next_ephemeral_port: u16,
    /// The thread identifier to be executed by the waking process.
    waking_thread_id: Option<ThreadId>,
    pub epolls: ValueIndex<EpollId, EpollInfo, FIZZLE_MAX_EPOLLS>,
    pub file_paths: FnvIndexMap<FilePath, FileId, FIZZLE_MAX_FILE_PATHS>,
    pub files: ValueIndex<FileId, FileBackend, FIZZLE_MAX_FILES>,
    pub sem_paths: FnvIndexMap<SemPath, SemaphoreId, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub semaphores: ValueIndex<SemaphoreId, SemaphoreInfo, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub pipes: ValueIndex<PipeId, PipeInfo, FIZZLE_MAX_PIPES>,
    pub message_queues: ValueIndex<MessageQueueId, MessageQueueInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
    // TODO: SO_REUSEPORT breaks this...
    pub socket_locations: FnvIndexMap<TransportAddress, SocketLocationInfo, FIZZLE_MAX_SOCKADDRS>,
    pub sockets: ValueIndex<SocketId, SocketState, FIZZLE_MAX_SOCKETS>,
    pub buffers: ValueIndex<BufferId, RingBuffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub transfer_fds: Option<ValueIndex<DescriptorId, FdInfo, FIZZLE_MAX_FDS>>,
    pub stdio: StdioBackend,
    // Polling infrastructure
    pub plugins: ValueIndex<PluginId, PluginInfo, FIZZLE_MAX_PLUGIN_STREAMS>,
    pub polled_events: ValueIndex<PolledId, PolledInfo, FIZZLE_MAX_POLLED_EVENTS>,
    pub pollers: ValueIndex<PollerId, PollerInfo, FIZZLE_MAX_POLLERS>,
    pub ready: Queue<ReadyInfo, FIZZLE_MAX_QUEUED_READY_POLLERS>,
}

impl InterprocessState {
    // TODO: initialize() is unsafe--whenever we change the fields in InterprocessState, it becomes
    // unsound until we add the corresponding definition. We should really change it to a trait +
    // proc macro derive.
    /// Takes an uninitialized InterprocessState and initializes it in place.
    unsafe fn initialize(state: *mut InterprocessState) {
        *ptr::addr_of_mut!((*state).next_process_id) = ProcessId::from(1);
        *ptr::addr_of_mut!((*state).next_stream_id) = StreamId::from(0);
        *ptr::addr_of_mut!((*state).next_ephemeral_port) = FIZZLE_EPHEMERAL_PORT_START;
        *ptr::addr_of_mut!((*state).waking_thread_id) = None;
        ValueIndex::initialize(ptr::addr_of_mut!((*state).epolls));
        *ptr::addr_of_mut!((*state).file_paths) = FnvIndexMap::new();
        ValueIndex::initialize(ptr::addr_of_mut!((*state).files));
        *ptr::addr_of_mut!((*state).sem_paths) = FnvIndexMap::new();
        ValueIndex::initialize(ptr::addr_of_mut!((*state).semaphores));
        ValueIndex::initialize(ptr::addr_of_mut!((*state).pipes));
        ValueIndex::initialize(ptr::addr_of_mut!((*state).message_queues));
        ValueIndex::initialize(ptr::addr_of_mut!((*state).message_queues));
        *ptr::addr_of_mut!((*state).socket_locations) = FnvIndexMap::new();
        ValueIndex::initialize(ptr::addr_of_mut!((*state).sockets));
        ValueIndex::initialize(ptr::addr_of_mut!((*state).buffers));
        *ptr::addr_of_mut!((*state).transfer_fds) = None;
        ValueIndex::initialize(ptr::addr_of_mut!((*state).plugins));
        ValueIndex::initialize(ptr::addr_of_mut!((*state).polled_events));
        ValueIndex::initialize(ptr::addr_of_mut!((*state).pollers));
        *ptr::addr_of_mut!((*state).ready) = Queue::new();
        *ptr::addr_of_mut!((*state).stdio) = StdioBackend::Sink;

    }

    fn load_config_mappings(&mut self, endpoints: Vec<PluginConfigEndpoint>) {
        for endpoint in endpoints {
            for _ in 0..endpoint.num_streams {
                match endpoint.endpoint_variant.clone() {
                    IoEndpointVariant::Stdio => self.stdio = match endpoint.emulation_type {
                        IoEmulationType::Feedback => StdioBackend::Feedback(StandardFeedback {
                            buf: self.buffers.put(RingBuffer::new()),
                            read_polled: self.polled_events.put(PolledInfo::new()),
                            write_polled: self.polled_events.put(PolledInfo::new_raised()),
                        }),
                        IoEmulationType::Plugin(plugin_id) => StdioBackend::Plugin(StandardPlugin {
                            plugin_id,
                            read_buf: self.buffers.put(RingBuffer::new()),
                            read_polled: self.polled_events.put(PolledInfo::new()),
                            write_buf: self.buffers.put(RingBuffer::new()),
                            write_polled: self.polled_events.put(PolledInfo::new_raised()),
                        }),
                        IoEmulationType::Sink =>StdioBackend::Sink,
                        IoEmulationType::NullSink => StdioBackend::NullSink,
                        IoEmulationType::Fuzz => StdioBackend::Fuzz(0),
                        IoEmulationType::Passthrough => StdioBackend::Passthrough,
                    },
                    IoEndpointVariant::File(pathbuf) => {
                        let path =
                            FilePath::from_raw_bytes(pathbuf.as_os_str().as_bytes()).unwrap();
                        let file_id = self.files.put(match endpoint.emulation_type {
                            IoEmulationType::Feedback => FileBackend::Feedback(StandardFeedback {
                                buf: self.buffers.put(RingBuffer::new()),
                                read_polled: self.polled_events.put(PolledInfo::new()),
                                write_polled: self.polled_events.put(PolledInfo::new_raised()),
                            }),
                            IoEmulationType::Plugin(plugin_id) => FileBackend::Plugin(StandardPlugin {
                                plugin_id,
                                read_buf: self.buffers.put(RingBuffer::new()),
                                read_polled: self.polled_events.put(PolledInfo::new()),
                                write_buf: self.buffers.put(RingBuffer::new()),
                                write_polled: self.polled_events.put(PolledInfo::new_raised()),
                            }),
                            IoEmulationType::Sink => FileBackend::Sink,
                            IoEmulationType::NullSink => FileBackend::NullSink,
                            IoEmulationType::Fuzz => FileBackend::Fuzz(0),
                            IoEmulationType::Passthrough => FileBackend::Passthrough,
                        });
                        self.file_paths.insert(path, file_id).unwrap();
                    }
                    IoEndpointVariant::TcpServer(addr) => {
                        let backend = match endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(plugin_id) => ServerBackend::Plugin(plugin_id),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(0),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.add_server(TransportAddress::Tcp(addr), backend)
                    }
                    IoEndpointVariant::TcpClient(addr) => {
                        let backend = match endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(0),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        self.add_pending_client(TransportAddress::Tcp(addr), backend)
                    }
                    IoEndpointVariant::UdpServer(addr) => {
                        let backend = match endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(plugin_id) => ServerBackend::Plugin(plugin_id),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(0),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.add_server(TransportAddress::Udp(addr), backend)
                    }
                    IoEndpointVariant::UdpClient(addr) => {
                        let backend = match endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(0),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        self.add_pending_client(TransportAddress::Udp(addr), backend)
                    }
                    IoEndpointVariant::SctpServer(addr) => {
                        let backend = match endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(plugin_id) => ServerBackend::Plugin(plugin_id),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(0),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.add_server(TransportAddress::Sctp(addr), backend)
                    }
                    IoEndpointVariant::SctpClient(addr) => {
                        let backend = match endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(0),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        self.add_pending_client(TransportAddress::Sctp(addr), backend)
                    }
                    _ => panic!("unimplemented IoEndpoint type"),
                }
            }
        }
    }

    pub fn add_pending_client(&mut self, rem_addr: TransportAddress, backend: PendingBackend) {
        let client_socket_id = self
            .sockets
            .put(SocketState::PendingConnection(PendingSocket {
                rem_addr,
                backend,
                next_pending: None,
            }));

        // Add the client to the pending client chain, if applicable
        match self.socket_locations.get_mut(&rem_addr) {
            None => {
                let polled_id = self
                    .polled_events
                    .put(PolledInfo::new());
                self.socket_locations
                    .insert(
                        rem_addr,
                        SocketLocationInfo {
                            bound_socket: None,
                            pending: Some(PendingInfo {
                                client: client_socket_id,
                                poll: polled_id,
                            }),
                        },
                    )
                    .unwrap();
            }
            Some(location_info) => match location_info.pending {
                Some(PendingInfo { mut client, .. }) => {
                    while let Some(SocketState::PendingConnection(PendingSocket {
                        next_pending: Some(id),
                        ..
                    })) = self.sockets.get(client)
                    {
                        client = *id;
                    }
                    let SocketState::PendingConnection(PendingSocket {
                        next_pending: next_awaiting,
                        ..
                    }) = &mut self.sockets.get_mut(client).unwrap()
                    else {
                        panic!("unexpected internal fizzle state--chain of awaiting clients had invalid socket variant")
                    };

                    *next_awaiting = Some(client_socket_id);
                }
                None => {
                    let polled_id = self
                        .polled_events
                        .put(PolledInfo::new());
                    location_info.pending = Some(PendingInfo {
                        client: client_socket_id,
                        poll: polled_id,
                    });
                }
            },
        }
    }

    pub fn add_server(&mut self, transport_addr: TransportAddress, backend: ServerBackend) {
        // Create a new polled instance for listeners waiting to accept connections
        let connect_polled_id = self.polled_events.put(PolledInfo::new());

        let socket_id = self.sockets.put(SocketState::Server(ServerSocket {
            backend,
            local_addr: transport_addr,
            connecting: Queue::new(),
            ready_to_connect: connect_polled_id,
        }));

        match self.socket_locations.get_mut(&transport_addr) {
            None => {
                self.socket_locations
                    .insert(
                        transport_addr,
                        SocketLocationInfo {
                            bound_socket: Some(socket_id),
                            pending: None,
                        },
                    )
                    .unwrap();
            }
            Some(location_info) => location_info.bound_socket = Some(socket_id),
        };
    }

    pub fn add_plugin(&mut self, endpoint: IoEndpointVariant, module_id: PluginModuleId) -> PluginId {
        let stream = self.next_stream_id;
        self.next_stream_id = StreamId::from(usize::from(stream) + 1);

        let read_buf = self.buffers.put(RingBuffer::default());
        let read_polled = self.polled_events.put(PolledInfo::new());
        let write_buf = self.buffers.put(RingBuffer::default());
        let write_polled = self.polled_events.put(PolledInfo::new_raised());

        let plugin_id = self.plugins.put(PluginInfo {
            endpoint,
            stream,
            write_buf,
            write_polled,
            read_buf,
            read_polled,
            module_id,
        });

        plugin_id
    }

    pub fn next_ephemeral_address(
        &mut self,
        family: AddressFamily,
        protocol: TransportProtocol,
    ) -> TransportAddress {
        let addr = SocketAddr::new(
            match family {
                AddressFamily::Ipv4 => IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), // TODO: enable configuration to specify this address
                AddressFamily::Ipv6 => IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)),
            },
            self.next_ephemeral_port,
        );
        if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
            self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
        } else {
            self.next_ephemeral_port += 1;
        }

        match protocol {
            TransportProtocol::Tcp => TransportAddress::Tcp(addr),
            TransportProtocol::Udp => TransportAddress::Udp(addr),
            TransportProtocol::Sctp => TransportAddress::Sctp(addr),
        }
    }

    /// Assigns the next available process ID and increments it internally.
    pub fn assign_process_id(&mut self) -> ProcessId {
        let process_id = self.next_process_id;
        self.next_process_id = ProcessId::from(usize::from(process_id) + 1);
        process_id
    }

    /// Marks the given process/thread pair as having further work to execute.
    pub fn mark_worker_ready(&mut self, worker_id: WorkerId) {
        self.ready.enqueue(ReadyInfo::Worker(worker_id)).unwrap();
    }

    /// This method returns `Ok` if the file was created, and `Err` if a file already
    /// exists at the given path.
    pub fn create_file(&mut self, path: FilePath) -> Result<FileId, FileId> {
        match self.file_paths.get(&path) {
            Some(&id) => Err(id),
            None => {
                let buf = self.buffers.put(RingBuffer::new());
                let read_polled = self.polled_events.put(PolledInfo::new());
                let write_polled = self.polled_events.put(PolledInfo::new_raised());
                let file_id = self.files.put(FileBackend::Feedback(StandardFeedback {
                    buf,
                    read_polled,
                    write_polled,
                }));
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

#[derive(Debug)]
pub struct BarrierInfo {
    pub curr: Vec<ThreadId>,
    pub needed: usize,
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
    pub read_polled: PolledId,
    pub write_polled: PolledId,
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
pub struct MessageQueueInfo {}


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
