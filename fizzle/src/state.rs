use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::io::Write;
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::{Deref, DerefMut};
use std::os::unix::ffi::OsStrExt;
use std::process::Command;
use std::thread::ThreadId;
use std::{array, env, mem, process, ptr, thread};

use fizzle_common::io::{
    AddressFamily, SocketAddrUnix, TransportAddress, TransportProtocol, MAX_PATH_LEN,
};
use fizzle_common::path::{FilePath, SemPath};
use fizzle_common::storage::Buffer;
use fizzle_plugin::{IoEndpointVariant, StreamId};
use fxhash::FxBuildHasher;
use heapless::{Deque, FnvIndexMap, FnvIndexSet};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::arena::{KeyedArena, Rc};
use crate::constants::*;
use crate::handlers::barrier::{BarrierInfo, BarrierPtr};
use crate::handlers::buffer::BufferId;
use crate::handlers::condvar::CondVarPtr;
use crate::handlers::descriptor::{DescriptorId, DescriptorInfo, FdResource};
use crate::handlers::directory::DirectoryId;
use crate::handlers::epoll::{EpollId, EpollInfo};
use crate::handlers::eventfd::{EventfdId, EventfdInfo};
use crate::handlers::file::{FileId, FileObject, FilePtr};
use crate::handlers::fuzz_endpoint::{FuzzEndpointId, FuzzEndpointInfo};
use crate::handlers::message_queue::{MessageQueueId, MessageQueueInfo};
use crate::handlers::mutex::MutexPtr;
use crate::handlers::pipe::{PipeId, PipeInfo};
use crate::handlers::plugin::{PluginEndpointId, PluginInfo};
use crate::handlers::plugin_module::PluginId;
use crate::handlers::polled::{PolledId, PolledInfo};
use crate::handlers::poller::{PollerId, PollerInfo};
use crate::handlers::process::ProcessId;
use crate::handlers::rwlock::{RwLockInfo, RwLockPtr};
use crate::handlers::semaphore::{SemaphoreId, SemaphoreInfo, SemaphorePtr};
use crate::handlers::signal::{ProcSigInfo, SignalHandlers, SignalSet, ThreadSigInfo};
use crate::handlers::socket::{
    PendingInfo, PendingSocket, ServerSocket, SocketId, SocketState, TransportLocationInfo,
};
use crate::handlers::spinlock::SpinlockPtr;
use crate::handlers::thread::PThreadRoutine;
use crate::once::SeqOnceCell;
use crate::plugins::{IoEmulationType, PluginEndpoint, Plugins};
use crate::semaphore::Semaphore;
use crate::comptime;

use crate::backend::{FileBackend, PendingBackend, ServerBackend, StandardFeedback, StdioBackend};

pub use private::FizzleSingleton;

mod private {
    pub struct FizzleSingleton {
        /// Empty private field to ensure `FizzleSingleton` isn't constructed outside of
        /// `fizzle_singleton()`.
        _private: (),
    }

    impl FizzleSingleton {
        pub(super) fn new() -> Self {
            FizzleSingleton { _private: () }
        }
    }
}

type Descriptors = KeyedArena<DescriptorId, DescriptorInfo, FIZZLE_MAX_FDS>;

static FIZZLE_STATE: SeqOnceCell<RefCell<FizzleState>> = SeqOnceCell::new();

static THREAD_LOCKS: SeqOnceCell<[RefCell<Option<Semaphore>>; FIZZLE_MAX_THREADS]> =
    SeqOnceCell::new();

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = const { RefCell::new(false) };
}

// TODO: mask SIGCHLD, SIGPIPE responses and manually implement
// TODO: add a signalfd that can handle SIGCHLD responses

/// Marks the thread as currently executing within a fizzle handler.
pub fn set_entered_handler(entered: bool) {
    ENTERED_HANDLER.with(|e| {
        *e.borrow_mut() = entered;
    });
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

/// Produces a new `FizzleSingleton` instance that can be used to acquire global state in a safe manner.
///
/// WARNING: this function SHOULD NOT be used in any methods other than a) the hook macro, or b)
/// those that create new threads, such as `pthread_create`. The `FizzleSingleton` is designed to
/// ensure that the global `FIZZLE_STATE` variable is never mutably referenced more than once. A
/// single instantiation of it is provided for each LD_PRELOAD hook; this instance is passed around
/// and is meant to be the _sole_ means of accessing global state.
///
/// WARNING 2: the `FizzleSingleton` prevents mutable aliasing within a single-threaded context, but
/// it cannot inherently prevent mutable access to data by multiple threads. Thread creation hooks
/// and scheduling routines need to ensure that any acquired `FizzGuard` instances are dropped prior
/// to another thread acquiring the global state.
pub unsafe fn fizzle_singleton() -> FizzleSingleton {
    FizzleSingleton::new()
}

impl FizzleSingleton {
    /// Creates the FizzleState instance for the given process.
    ///
    /// This function is called at most once per process, and is one of the very first instatiations
    /// Fizzle runs. As such, it contains methods that need to be called on startup (logging, handling
    /// children, etc.) and must be done prior to any other initialization routines.
    fn instantiate() -> RefCell<FizzleState> {
        // Initialize logger to print PID and TID with each message
        env_logger::Builder::from_default_env()
            .format(|buf, record| {
                writeln!(
                    buf,
                    "[PID({:8})|{:4?}|{}] {}",
                    process::id(),
                    thread::current().id(),
                    record.level().as_str().to_uppercase(),
                    record.args()
                )
            })
            .init();
        log::info!("Logger initialized");

        // Set signal mask to be inherited by all threads/processes of Fizzle
        unsafe {
            let new_set = (SignalSet::SIGPIPE | SignalSet::SIGCHLD).to_sigset();
            let mut old_set = SignalSet::empty().to_sigset();
            assert_eq!(
                libc::pthread_sigmask(
                    libc::SIG_SETMASK,
                    ptr::addr_of!(new_set),
                    ptr::addr_of_mut!(old_set)
                ),
                0
            );
        }

        // Clean up child processes if the parent is ever killed
        unsafe {
            assert_eq!(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM), 0);
        }

        // Allocate and instantiate the FizzleState
        let ctx = RefCell::new(FizzleState::new());

        log::info!("FizzleSingleton::instantiate() complete");
        ctx
    }

    /// Acquires the global shared state for mutable access.
    ///
    /// This access does not involve any atomic or locking operations.
    pub fn acquire(&mut self) -> RefMut<'_, FizzleState> {
        FIZZLE_STATE.get_or_init(Self::instantiate).borrow_mut()
    }

    fn init_thread_lock(&mut self, thread_id: &ThreadId) {
        let locks = THREAD_LOCKS
            .get_or_situate(|uninit| uninit.write(array::from_fn(|_| RefCell::new(None))));
        let thread_idx = crate::handlers::thread::index_of_thread(thread_id);
        let mut sem_opt = locks[thread_idx].borrow_mut();
        let sem_opt_deref = sem_opt.deref_mut();

        // TODO: this seems to behave safely, but check with Miri
        unsafe {
            let uninit_sem = (*(ptr::from_mut(sem_opt_deref)
                as *mut Option<MaybeUninit<Semaphore>>))
                .insert(MaybeUninit::uninit());
            Semaphore::initialize(uninit_sem, false, 0);
        }
        drop(sem_opt);
    }

    /// Destroys the thread lock of the calling thread.
    pub fn destroy_thread_lock(&mut self) {
        // Invariant: THREAD_LOCKS is instantiated via `init_thread_lock()` before this is called
        let locks = THREAD_LOCKS.get().unwrap();
        let thread_idx = crate::handlers::thread::index_of_thread(&thread::current().id());
        *locks[thread_idx].borrow_mut() = None;
    }

    pub fn thread_lock(&mut self, thread_id: &ThreadId) -> Ref<'_, Option<Semaphore>> {
        // Invariant: THREAD_LOCKS is instantiated via `init_thread_lock()` before this is called
        let locks = THREAD_LOCKS.get().unwrap();

        let thread_idx = crate::handlers::thread::index_of_thread(thread_id);
        locks[thread_idx].borrow()
    }

    pub fn init_new_thread(&mut self) {
        let thread_id = thread::current().id();
        let mut state = self.acquire();
        state
            .local
            .pthreads
            .insert(unsafe { libc::pthread_self() }, thread_id);
        state
            .local
            .signals
            .insert(thread_id, ThreadSigInfo::default());
        drop(state);
        self.init_thread_lock(&thread::current().id());
    }
}

#[derive(Debug)]
pub struct FizzleState {
    pub local: ProcessLocalState,
    pub global: &'static mut InterprocessState,
}

impl FizzleState {
    /// Allocates a new instance of Fizzle's in-process/shared state.
    fn new() -> Self {
        // NOTE: must go before `allocate_global_memory`, as this env variable gets set within it.
        let is_child_process = matches!(env::var(FIZZLE_MEMORY_ENV), Ok(_));

        // Allocate shared memory for process-shared state
        let global_uninit = Self::allocate_global_memory();

        // Initialize process-shared state
        let global = if is_child_process {
            unsafe { global_uninit.assume_init_mut() }
        } else {
            InterprocessState::situate(global_uninit)
        };

        // Initialize process-local state
        let local = ProcessLocalState::new();

        Self { local, global }
    }

    /// Maps the memory to Fizzle's global shared state, allocating such memory if this is the
    /// primary process.
    fn allocate_global_memory() -> &'static mut MaybeUninit<InterprocessState> {
        let size = mem::size_of::<InterprocessState>();
        let is_singleprocess =
            matches!(env::var(FIZZLE_SINGLEPROCESS_ENV), Ok(s) if s.as_str() == "1");

        if is_singleprocess {
            unsafe {
                let location = libc::mmap(
                    ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                );

                if location == libc::MAP_FAILED {
                    panic!(
                        "failed to mmap global memory (errno {})",
                        *libc::__errno_location()
                    )
                }

                return &mut *(location as *mut MaybeUninit<InterprocessState>);
            }
        }

        // Shared memory doesn't play well with the forkserver, so we need to make sure that
        // processes are forked *before* any shared memory is created.
        #[cfg(feature = "afl")]
        unsafe {
            crate::__afl_manual_init();
        }

        let (key, flags) = match env::var(FIZZLE_MEMORY_ENV) {
            Ok(var) => {
                log::debug!("attaching to already-created shared memory");
                (var.parse().unwrap(), (libc::S_IRUSR | libc::S_IWUSR) as i32)
            }
            Err(_) => unsafe {
                let key = libc::getpid();
                env::set_var(FIZZLE_MEMORY_ENV, key.to_string());
                log::debug!("allocating public shared memory object with key {}", key);
                (
                    key,
                    (libc::S_IRUSR | libc::S_IWUSR) as i32 | libc::IPC_CREAT | libc::IPC_EXCL,
                )
            },
        };

        unsafe {
            let shmid = libc::shmget(key, size, flags);
            assert!(
                shmid >= 0,
                "shared memory creation for key {} failed (errno {})",
                key,
                *libc::__errno_location()
            );
            log::debug!("shared memory allocated with shmid {}", shmid);

            let location = libc::shmat(shmid, ptr::null_mut(), 0);
            assert!(
                location as isize != -1,
                "mapping shared memory failed (errno {})",
                *libc::__errno_location()
            );

            &mut *(location as *mut MaybeUninit<InterprocessState>)
        }
    }

    /// Initializes any default values for global interprocess and/or local process state.
    ///
    /// This method should only be called once per created process within the `Scheduler`.
    #[cold]
    pub fn initialize_state(&mut self) {
        assert!(!self.local.is_initialized);

        if !self.global.is_initialized {
            self.initialize_global();
        }

        self.initialize_local();

        if self.local.process_id.is_main_process() {
            // Additionally initialize plugin state
            // NOTE: must be done _after_ global/local state is initialized.
            self.local.main_state = Some(MainProcessState::new());
        }
    }

    /// Initializes local process state.
    ///
    /// This method should only be called once per created process within the `Scheduler`.
    fn initialize_local(&mut self) {
        let (local, global) = self.split();

        assert!(!local.is_initialized);
        local.is_initialized = true;

        // Assign the process ID to be used for this process
        let process_id = global.assign_process_id();
        local.process_id = process_id;

        // Insert the current (main) pthread into `pthreads`
        local.pthreads.insert(unsafe { libc::pthread_self() }, thread::current().id());

        // Inherit signal handlers
        if let Some(handlers) = global.inherited_handlers.take() {
            global.signals.allocate_with_key(process_id, ProcSigInfo::inherit(handlers));
        } else {
            global.signals.allocate_with_key(process_id, ProcSigInfo::new());
        }

        // Inherit blocked sigmask
        if let Some(sigmask) = global.inherited_sigmask.take() {
            local.signals.insert(thread::current().id(), ThreadSigInfo::inherit(sigmask));
        } else {
            local.signals.insert(thread::current().id(), ThreadSigInfo::new());
        }

        // Inherit parent's file descriptors
        if let Some(transfer_fds) = self.global.transfer_fds.take().map(Box::new) {
            // Generally, moving `KeyedArena`s is unsafe because `Rc<>` references rely on arenas
            // remaining in a fixed location in memory. However, `fds` never makes use of these
            // references, so this is safe to do.
            self.local.fds = transfer_fds;
        } else {
            // Initialize parent's file descriptors
            self.local.fds.allocate_with_key(
                DescriptorId::from_raw_fd(0),
                DescriptorInfo::new(FdResource::Stdin),
            )
            .unwrap();
            self.local.fds.allocate_with_key(
                DescriptorId::from_raw_fd(1),
                DescriptorInfo::new(FdResource::Stdout),
            )
            .unwrap();
            self.local.fds.allocate_with_key(
                DescriptorId::from_raw_fd(2),
                DescriptorInfo::new(FdResource::Stderr),
            )
            .unwrap();
        }

        // Initialize this process's global lock
        let sem_opt = &mut self.global.process_locks[usize::from(process_id)];
        // TODO: this seems to behave safely, but check with Miri
        unsafe {
            let uninit_sem = (*(ptr::from_mut(sem_opt) as *mut Option<MaybeUninit<Semaphore>>))
                .insert(MaybeUninit::uninit());
            Semaphore::initialize(uninit_sem, true, 0);
        }
    }

    /// Initializes global interprocess state.
    ///
    /// This method should only be called once in the lifetime of the Fizzle harness.
    #[cold]
    fn initialize_global(&mut self) {
        assert!(!self.global.is_initialized);
        self.global.is_initialized = true;

        // Initialize plugins for the main process
        let mut plugins: Box<MaybeUninit<Plugins>> = Box::new(MaybeUninit::uninit());
        // This needs to remain fixed in a location, so we use a Box with in-place initialization
        unsafe { Plugins::initialize(plugins.as_mut_ptr()) };
        self.local.plugins = Some(unsafe { plugins.assume_init() });

        // Initialize plugin endpoints
        let mut endpoints = Vec::new();
        comptime::populate_plugins(&mut endpoints, self.local.plugins.as_mut().unwrap());
        self.global.load_config_mappings(endpoints);
    }

    /// Returns both local and global state in separate lifetimes that can be used simultaneously.
    ///
    /// This method is used specifically to handle a particular lifetime issue in the Scheduler.
    pub fn split(&mut self) -> (&mut ProcessLocalState, &mut InterprocessState) {
        (&mut self.local, self.global)
    }

    /// Indicates whether the given polled event is ready to be acted on.
    pub fn polled_is_ready(&mut self, polled_id: &Rc<PolledId>) -> bool {
        let polled = self.global.polled_events.get(polled_id).unwrap();
        polled.event_raised
    }

    /// Marks the given polled event as ready.
    ///
    /// If not already raised, this method will push_back a poller waiting on this polled event
    /// (if such a poller exists).
    pub fn raise_polled(&mut self, polled_id: &Rc<PolledId>) {
        self.global.raise_polled(polled_id);
    }

    // if buffer is empty, then call this
    pub fn lower_polled(&mut self, polled_id: &Rc<PolledId>) {
        let polled = self.global.polled_events.get_mut(polled_id).unwrap();
        debug_assert!(polled.event_raised);
        polled.event_raised = false;
    }

    /// Creates a new poller for the currently executing worker.
    pub fn new_poller(&mut self) -> Rc<PollerId> {
        let worker_id = self.current_worker_id();

        self.global
            .pollers
            .allocate(PollerInfo {
                worker_id,
                polled_events: heapless::Vec::new(),
                raised_events: heapless::FnvIndexSet::new(),
            })
            .unwrap()
    }

    /// Registers `poller_id` as waiting on `polled_id`.
    pub fn register_poller(&mut self, poller_id: Rc<PollerId>, polled_id: Rc<PolledId>) {
        let poller = self.global.pollers.get_mut(&poller_id).unwrap();
        poller.polled_events.push(polled_id.clone()).unwrap();
        let polled = self.global.polled_events.get_mut(&polled_id).unwrap();
        debug_assert!(!polled.event_raised);
        polled.pollers.push(poller_id).unwrap();
    }

    // Ugh. This looks like O(n^2)...
    /// Deletes the given poller, removing any references to it from `Polled` objects.
    pub fn delete_poller(&mut self, poller_id: Rc<PollerId>) {
        let poller = self.global.pollers.get_mut(&poller_id).unwrap();

        if poller.deref().in_raised_queue() {
            // TODO: make queue indexable in future to improve speed here

            // Remove the poller from the ready queue, leaving the others in the same order
            for _ in 0..self.global.ready.len() {
                let ready = self.global.ready.pop_front().unwrap();
                if let ReadyInfo::Poller(current_poller_id) = &ready {
                    if *current_poller_id != poller_id {
                        self.global.ready.push_back(ready).unwrap();
                    }
                }
            }
        }

        // Remove the poller from each polled instance it was registered to
        for polled_id in poller.polled_events.iter() {
            let polled = self.global.polled_events.get_mut(&polled_id).unwrap();
            for i in 0..polled.pollers.len() {
                if *polled.pollers.get(i).unwrap() == poller_id {
                    polled.pollers.remove(i);
                }
            }
        }
    }

    pub fn current_worker_id(&mut self) -> WorkerId {
        WorkerId {
            process_id: self.local.process_id,
            thread_id: thread::current().id(),
        }
    }

    // TODO: need to figure out where this fits...
    pub fn copy_exec_fds(&mut self) {
        let fds = self.global.transfer_fds.insert(self.local.fds.as_ref().clone());
        let mut downref_keys = Vec::new();

        for key in fds.keys() {
            if let Some(DescriptorInfo {
                close_on_exec: true,
                ..
            }) = fds.get(&key)
            {
                downref_keys.push(key);
            }
        }

        for key in downref_keys {
            fds.downref(&key);
        }
    }
}

/// State specific to the first (root) process instantiated by Fizzle.
pub struct MainProcessState {
    pub onstartup_commands: Vec<Command>,
    pub onready_commands: Vec<Command>,
    pub plugins: Box<Plugins>,
}

impl Debug for MainProcessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainProcessState")
            .field("onstartup_commands", &self.onstartup_commands)
            .field("awaiting_thread_death", &self.onready_commands)
            .field("plugins", &"<opaque>")
            .finish()
    }
}

impl MainProcessState {
    fn new() -> Self {
        let mut onstartup_commands = Vec::new();
        let mut onready_commands = Vec::new();

        // Initialize immediate ("onstartup") commands
        comptime::populate_onstartup_processes(&mut onstartup_commands);

        // Initialize delayed ("onready") commands
        comptime::populate_onready_processes(&mut onready_commands);

        // Initialize plugins--these need to remain fixed in memory, so we use a Box with in-place initialization.
        let mut plugins: Box<MaybeUninit<Plugins>> = Box::new_uninit();
        unsafe {
            Plugins::initialize(plugins.as_mut_ptr());
            Self {
                onstartup_commands,
                onready_commands,
                plugins: plugins.assume_init(),
            }
        }
    }
}

pub struct ProcessLocalState {
    /// Indicates whether the given process-local state has completed initialization routines.
    ///
    /// When `ProcessLocalState` is first allocated and instantiated, this is set to `false`. Once
    /// `FizzleState::initialize_local()` is called, this is set to `true`.
    pub is_initialized: bool,
    /// State associated with the main process (e.g. the first process instantiated with the Fizzle harness).
    pub main_state: Option<MainProcessState>,
    /// A thread that has received a cancellation request.
    pub cancelling: Option<ThreadId>,

    pub process_id: ProcessId,
    /// Indicates that the thread being awoken should be immediately cancelled and delegate execution back to this thread.
    /// Plugin modules for handling I/O.
    ///
    /// This field is only `Some` in the parent process; all other processes must delegate control
    /// flow to it in order to handle plugin I/O.
    pub plugins: Option<Box<Plugins>>,
    /// A supplamentary thread used to reap exiting threads.
    pub reaper: Option<ThreadId>,
    pub fds: Box<Descriptors>,
    pub dirs: KeyedArena<DirectoryId, FilePath<MAX_PATH_LEN>, FIZZLE_MAX_DIRS>,
    pub barriers: HashMap<BarrierPtr, BarrierInfo, FxBuildHasher>,
    pub condvars: HashMap<CondVarPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub named_semaphores: HashMap<SemaphorePtr, Rc<SemaphoreId>>,
    /// Files specifically designated as being emulated.
    pub file_objs: HashMap<FilePtr, FileObject, FxBuildHasher>,
    pub mutexes: HashMap<MutexPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub rwlocks: HashMap<RwLockPtr, RwLockInfo, FxBuildHasher>,
    pub semaphores: HashMap<SemaphorePtr, SemaphoreInfo>,
    pub spinlocks: HashMap<SpinlockPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub pthreads: HashMap<libc::pthread_t, ThreadId, FxBuildHasher>,
    pub pthread_cleanup: HashMap<ThreadId, VecDeque<PThreadRoutine>, FxBuildHasher>,
    pub pthread_keys: HashMap<libc::pthread_key_t, PThreadRoutine, FxBuildHasher>,
    pub pthread_key_values: HashMap<
        libc::pthread_key_t,
        HashMap<ThreadId, *mut libc::c_void, FxBuildHasher>,
        FxBuildHasher,
    >,
    pub futex_waiters: HashMap<*const u32, VecDeque<(u32, ThreadId)>, FxBuildHasher>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,

    pub signals: HashMap<ThreadId, ThreadSigInfo, FxBuildHasher>,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath<MAX_PATH_LEN>,
}

impl Debug for ProcessLocalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FizzLocal")
            .field("is_initialized", &self.is_initialized)
            .field("main_state", &self.main_state)
            .field("process_id", &self.process_id)
            .field("cancelling", &self.cancelling)
            .field("reaper", &self.reaper)
            .field("fds", &self.fds)
            .field("dirs", &self.dirs)
            .field("barriers", &self.barriers)
            .field("condvars", &self.condvars)
            .field("named_semaphores", &self.named_semaphores)
            .field("file_objs", &self.file_objs)
            .field("mutexes", &self.mutexes)
            .field("rwlocks", &self.rwlocks)
            .field("semaphores", &self.semaphores)
            .field("spinlocks", &self.spinlocks)
            .field("pthreads", &self.pthreads)
            .field("pthread_cleanup", &self.pthread_cleanup)
            .field("pthread_keys", &self.pthread_keys)
            .field("pthread_key_values", &self.pthread_key_values)
            .field("terminated_threads", &self.terminated_threads)
            .field("awaiting_thread_death", &self.awaiting_thread_death)
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

impl ProcessLocalState {
    fn new() -> Self {
        let working_directory =
            FilePath::from_raw_bytes(env::current_dir().unwrap().as_os_str().as_bytes()).unwrap();

        Self {
            is_initialized: false,
            main_state: None,
            cancelling: None,
            process_id: ProcessId::main_process(),
            plugins: None,
            reaper: None,
            fds: Box::new(KeyedArena::new()),
            dirs: Default::default(),
            barriers: HashMap::default(),
            condvars: HashMap::default(),
            file_objs: HashMap::default(),
            mutexes: HashMap::default(),
            named_semaphores: HashMap::default(),
            rwlocks: HashMap::default(),
            semaphores: HashMap::default(),
            spinlocks: HashMap::default(),
            pthreads: HashMap::default(),
            pthread_cleanup: HashMap::default(),
            pthread_keys: HashMap::default(),
            pthread_key_values: HashMap::default(),
            signals: HashMap::default(),
            futex_waiters: HashMap::default(),
            terminated_threads: HashSet::default(),
            working_directory,
            awaiting_thread_death: HashMap::default(),
        }
    }
}

#[derive(Debug)]
pub struct InterprocessState {
    /// Indicates whether the state has been properly initialized (not just instantiated).
    pub is_initialized: bool,

    /// The thread identifier to be executed by the waking process. This is `Some` if and only if
    /// a thread is currently about to be scheduled.
    pub waking_id: Option<ThreadId>,
    /// The thread/process identifier to be reaped. This is `Some` if and only if a thread/process
    /// is currently exiting.
    pub exiting_id: Option<WorkerId>,
    /// The process ID to be passed through a call to one of the `exec*` family of functions.
    /// 
    /// The pid of a process changes when `fork()` is called, but not when `exec*` is.
    pub passthrough_id: Option<ProcessId>, // TODO: implement
    /// The signal handlers that have been inherited from a parent process.
    pub inherited_handlers: Option<SignalHandlers>,
    /// The mask of blocked signals that have been inherited from a parent thread.
    pub inherited_sigmask: Option<SignalSet>, // TODO: implement `exec()` passthrough of IGN, but not handlers

    /// The thread/process identifier to be signalled with the given signal value. This is `Some`
    /// if and only if a thread is about to receive an outstanding signal.
    pub signal: Option<(SignalDestination, i32)>,
    /// Signal dispositions, handlers and blocked incoming signals for each process.
    pub signals: KeyedArena<ProcessId, ProcSigInfo, FIZZLE_MAX_PROCESSES>,

    // /// The list of subprocess that are meant to be spawned once all available work has been completed.
    pub plugin_worker: Option<WorkerId>,
    pub persistent_rounds: usize,
    pub next_process_id: ProcessId,
    /// The next StreamId available to be assigned to an emulated stream.
    pub next_stream_id: StreamId,
    /// The next ephemeral port to be assigned to a socket.
    pub next_ephemeral_port: u16,
    pub process_locks: [Option<Semaphore>; FIZZLE_MAX_PROCESSES],
    pub pids: FnvIndexMap<libc::pid_t, ProcessId, FIZZLE_MAX_PROCESSES>, // TODO: implement initialization and use of this
    pub gids: FnvIndexMap<libc::gid_t, ProcessId, FIZZLE_MAX_PROCESSES>, // TODO: implement initialization and use of this
    pub transfer_fds: Option<Descriptors>,
    pub shared_mem_initialized: bool,
    pub epolls: KeyedArena<EpollId, EpollInfo, FIZZLE_MAX_EPOLLS>,
    pub event_fds: KeyedArena<EventfdId, EventfdInfo, FIZZLE_MAX_EVENTFDS>,
    pub file_paths: FnvIndexMap<FilePath<MAX_PATH_LEN>, Rc<FileId>, FIZZLE_MAX_FILE_PATHS>,
    pub files: KeyedArena<FileId, FileBackend, FIZZLE_MAX_FILES>,
    pub sem_paths: FnvIndexMap<SemPath, Rc<SemaphoreId>, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub semaphores: KeyedArena<SemaphoreId, SemaphoreInfo, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub pipes: KeyedArena<PipeId, PipeInfo, FIZZLE_MAX_PIPES>,
    pub message_queues: KeyedArena<MessageQueueId, MessageQueueInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
    // TODO: SO_REUSEPORT breaks this...
    pub socket_locations:
        FnvIndexMap<TransportAddress, TransportLocationInfo, FIZZLE_MAX_SOCKADDRS>,
    pub sockets: KeyedArena<SocketId, SocketState, FIZZLE_MAX_SOCKETS>,
    pub buffers: KeyedArena<BufferId, Buffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub stdio: StdioBackend,
    // Polling infrastructure
    pub plugins: KeyedArena<PluginEndpointId, PluginInfo, FIZZLE_MAX_PLUGIN_STREAMS>,
    pub polled_events: KeyedArena<PolledId, PolledInfo, FIZZLE_MAX_POLLED_EVENTS>,
    pub pollers: KeyedArena<PollerId, PollerInfo, FIZZLE_MAX_POLLERS>,
    /// Pollers/Workers that can be immediately scheduled.
    pub ready: Deque<ReadyInfo, FIZZLE_MAX_QUEUED_READY_POLLERS>,
    /// Pollers/Workers that should be scheduled once the system has reached a halted state.
    pub delayed_ready: Deque<ReadyInfo, FIZZLE_MAX_QUEUED_READY_POLLERS>,
    pub fuzz_input: Buffer<FIZZLE_MAX_FUZZ_INPUT>,
    pub per_round_clients: heapless::Vec<PerRoundClientInfo, FIZZLE_MAX_PER_ROUND_ENDPOINTS>,
    pub per_round_endpoints: FnvIndexSet<Rc<SocketId>, FIZZLE_MAX_PER_ROUND_ENDPOINTS>,
    pub fuzz_endpoints: KeyedArena<FuzzEndpointId, FuzzEndpointInfo, FIZZLE_MAX_FUZZ_ENDPOINTS>,
    pub prefuzz_rng: rand::rngs::SmallRng,
}

impl InterprocessState {
    // TODO: situate() is unsafe--whenever we change the fields in InterprocessState, it becomes
    // unsound until we add the corresponding definition. We should really change it to a trait +
    // proc macro derive.
    /// Takes an uninitialized InterprocessState and initializes it in place.
    fn situate(state: &mut MaybeUninit<InterprocessState>) -> &mut InterprocessState {
        unsafe {
            let state = state.as_mut_ptr();
            *ptr::addr_of_mut!((*state).is_initialized) = false;

            *ptr::addr_of_mut!((*state).waking_id) = None;
            *ptr::addr_of_mut!((*state).exiting_id) = None;
            *ptr::addr_of_mut!((*state).passthrough_id) = None;
            *ptr::addr_of_mut!((*state).inherited_handlers) = None;
            *ptr::addr_of_mut!((*state).inherited_sigmask) = None;

            *ptr::addr_of_mut!((*state).signal) = None; 
            KeyedArena::initialize(ptr::addr_of_mut!((*state).signals));

            *ptr::addr_of_mut!((*state).plugin_worker) = None;
            *ptr::addr_of_mut!((*state).persistent_rounds) = FIZZLE_AFL_LOOP; // TODO: make configurable
            *ptr::addr_of_mut!((*state).next_process_id) = ProcessId::from(0);
            *ptr::addr_of_mut!((*state).next_stream_id) = StreamId::from(0);
            *ptr::addr_of_mut!((*state).next_ephemeral_port) = FIZZLE_EPHEMERAL_PORT_START;
            *ptr::addr_of_mut!((*state).process_locks) = array::from_fn(|_| None);

            *ptr::addr_of_mut!((*state).pids) = FnvIndexMap::new();
            *ptr::addr_of_mut!((*state).gids) = FnvIndexMap::new();

            *ptr::addr_of_mut!((*state).transfer_fds) = None;
            *ptr::addr_of_mut!((*state).shared_mem_initialized) = false;
            KeyedArena::initialize(ptr::addr_of_mut!((*state).epolls));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).event_fds));
            *ptr::addr_of_mut!((*state).file_paths) = FnvIndexMap::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).files));
            *ptr::addr_of_mut!((*state).sem_paths) = FnvIndexMap::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).semaphores));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).pipes));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).message_queues));
            *ptr::addr_of_mut!((*state).socket_locations) = FnvIndexMap::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).sockets));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).buffers));

            *ptr::addr_of_mut!((*state).stdio) = StdioBackend::Passthrough;
            KeyedArena::initialize(ptr::addr_of_mut!((*state).plugins));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).polled_events));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).pollers));
            *ptr::addr_of_mut!((*state).ready) = Deque::new();
            *ptr::addr_of_mut!((*state).delayed_ready) = Deque::new();
            *ptr::addr_of_mut!((*state).fuzz_input) = Buffer::new();
            *ptr::addr_of_mut!((*state).per_round_clients) = heapless::Vec::new();
            *ptr::addr_of_mut!((*state).per_round_endpoints) = FnvIndexSet::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).fuzz_endpoints));
            *ptr::addr_of_mut!((*state).prefuzz_rng) =
                SmallRng::seed_from_u64(0xABAD_5EED_ABAD_5EED_u64); // TODO: enable custom seed loading

            &mut (*state)
        }
    }

    fn load_config_mappings(&mut self, endpoints: Vec<PluginEndpoint>) {
        for endpoint in endpoints {
            for _ in 0..endpoint.num_streams {
                let endpoint_variant = endpoint.endpoint_variant.clone();
                match endpoint_variant {
                    IoEndpointVariant::Stdio => {
                        self.stdio = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => StdioBackend::Feedback(StandardFeedback {
                                buf: self.buffers.allocate(Buffer::new()).unwrap(),
                                read_polled: self
                                    .polled_events
                                    .allocate(PolledInfo::new())
                                    .unwrap(),
                                write_polled: self
                                    .polled_events
                                    .allocate(PolledInfo::new_raised())
                                    .unwrap(),
                            }),
                            IoEmulationType::Plugin(module_id) => {
                                StdioBackend::Plugin(self.add_plugin(
                                    endpoint.endpoint_variant.clone(),
                                    module_id.clone(),
                                ))
                            }
                            IoEmulationType::Sink => StdioBackend::Sink,
                            IoEmulationType::NullSink => StdioBackend::NullSink,
                            IoEmulationType::Fuzz => {
                                let fuzz_endpoint_id = self.add_fuzz_endpoint();
                                StdioBackend::Fuzz(fuzz_endpoint_id)
                            }
                            IoEmulationType::Passthrough => StdioBackend::Passthrough,
                        }
                    }
                    IoEndpointVariant::File(pathbuf) => {
                        let path =
                            FilePath::from_raw_bytes(pathbuf.as_os_str().as_bytes()).unwrap();

                        let file_id = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => self
                                .files
                                .allocate(FileBackend::Feedback(StandardFeedback {
                                    buf: self.buffers.allocate(Buffer::new()).unwrap(),
                                    read_polled: self
                                        .polled_events
                                        .allocate(PolledInfo::new())
                                        .unwrap(),
                                    write_polled: self
                                        .polled_events
                                        .allocate(PolledInfo::new_raised())
                                        .unwrap(),
                                }))
                                .unwrap(),
                            IoEmulationType::Plugin(module_id) => {
                                let backend = FileBackend::Plugin(self.add_plugin(
                                    endpoint.endpoint_variant.clone(),
                                    module_id.clone(),
                                ));
                                self.files.allocate(backend).unwrap()
                            }
                            IoEmulationType::Sink => {
                                self.files.allocate(FileBackend::Sink).unwrap()
                            }
                            IoEmulationType::NullSink => {
                                self.files.allocate(FileBackend::NullSink).unwrap()
                            }
                            IoEmulationType::Fuzz => {
                                let fuzz_endpoint_id = self.add_fuzz_endpoint();
                                let file_id = self
                                    .files
                                    .allocate(FileBackend::Fuzz(fuzz_endpoint_id))
                                    .unwrap();

                                file_id
                            }
                            IoEmulationType::Passthrough => {
                                self.files.allocate(FileBackend::Passthrough).unwrap()
                            }
                        };

                        self.file_paths.insert(path, file_id).unwrap();
                    }
                    IoEndpointVariant::TcpServer(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => ServerBackend::Plugin(
                                self.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(self.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.add_server(
                            TransportAddress::new_inet(addr, TransportProtocol::Tcp),
                            backend,
                        )
                    }
                    IoEndpointVariant::TcpClient(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => PendingBackend::Plugin(
                                self.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(self.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        let target_address =
                            TransportAddress::new_inet(addr, TransportProtocol::Tcp);
                        let source_address = self
                            .ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.per_round_clients
                                .push(PerRoundClientInfo {
                                    source_address,
                                    target_address,
                                    backend: match backend {
                                        PendingBackend::Fuzz(fuzz_endpoint_id) => {
                                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id)
                                        }
                                        PendingBackend::Plugin(plugin_id) => {
                                            PerRoundClientBackend::Plugin(plugin_id)
                                        }
                                        _ => unreachable!(),
                                    },
                                })
                                .unwrap();
                        } else {
                            self.add_pending_client(source_address, target_address, backend);
                        }
                    }
                    IoEndpointVariant::UdpServer(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => ServerBackend::Plugin(
                                self.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(self.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.add_server(
                            TransportAddress::new_inet(addr, TransportProtocol::Udp),
                            backend,
                        )
                    }
                    IoEndpointVariant::UdpClient(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => PendingBackend::Plugin(
                                self.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(self.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        let target_address =
                            TransportAddress::new_inet(addr, TransportProtocol::Udp);
                        let source_address = self
                            .ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.per_round_clients
                                .push(PerRoundClientInfo {
                                    source_address,
                                    target_address,
                                    backend: match backend {
                                        PendingBackend::Fuzz(fuzz_endpoint_id) => {
                                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id)
                                        }
                                        PendingBackend::Plugin(plugin_id) => {
                                            PerRoundClientBackend::Plugin(plugin_id)
                                        }
                                        _ => unreachable!(),
                                    },
                                })
                                .unwrap();
                        } else {
                            self.add_pending_client(source_address, target_address, backend);
                        }
                    }
                    IoEndpointVariant::SctpServer(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => ServerBackend::Plugin(
                                self.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(self.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.add_server(
                            TransportAddress::new_inet(addr, TransportProtocol::Sctp),
                            backend,
                        )
                    }
                    IoEndpointVariant::SctpClient(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => PendingBackend::Plugin(
                                self.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(self.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        let target_address =
                            TransportAddress::new_inet(addr, TransportProtocol::Sctp);
                        let source_address = self
                            .ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.per_round_clients
                                .push(PerRoundClientInfo {
                                    source_address,
                                    target_address,
                                    backend: match backend {
                                        PendingBackend::Fuzz(fuzz_endpoint_id) => {
                                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id)
                                        }
                                        PendingBackend::Plugin(plugin_id) => {
                                            PerRoundClientBackend::Plugin(plugin_id)
                                        }
                                        _ => unreachable!(),
                                    },
                                })
                                .unwrap();
                        } else {
                            self.add_pending_client(source_address, target_address, backend);
                        }
                    }
                    _ => panic!("unimplemented IoEndpoint type"),
                }
            }
        }
    }

    pub fn gen_random_bytes(&mut self, input: &mut [MaybeUninit<u8>]) {
        if self.fuzz_input.is_empty() {
            for b in input {
                *b = MaybeUninit::new(self.prefuzz_rng.gen());
            }
        } else {
            let data = self.fuzz_input.data();
            let mut idx = 0usize;
            for b in input {
                *b = MaybeUninit::new(data[idx]);
                idx = (idx + 1) % data.len();
            }
        }
    }

    /// Marks the given polled event as ready.
    ///
    /// If not already raised, this method will push_back a poller waiting on this polled event
    /// (if such a poller exists).
    fn raise_polled(&mut self, polled_id: &Rc<PolledId>) {
        let polled = self.polled_events.get_mut(polled_id).unwrap();
        if !polled.event_raised {
            polled.event_raised = true;
            let pollers = polled.pollers.clone();
            for poller in pollers {
                if !self.pollers.get(&poller).unwrap().in_raised_queue() {
                    self.ready
                        .push_back(ReadyInfo::Poller(poller.clone()))
                        .unwrap();
                }
                self.pollers
                    .get_mut(&poller)
                    .unwrap()
                    .raised_events
                    .insert(polled_id.clone())
                    .unwrap();
            }
        }
    }

    pub fn gen_random_array<const N: usize>(&mut self) -> [u8; N] {
        if self.fuzz_input.is_empty() {
            array::from_fn(|_| self.prefuzz_rng.gen())
        } else {
            let data = self.fuzz_input.data();
            array::from_fn(|i| data[i % data.len()])
        }
    }

    pub fn add_fuzz_endpoint(&mut self) -> Rc<FuzzEndpointId> {
        let read_polled = self.polled_events.allocate(PolledInfo::new()).unwrap();
        self.fuzz_endpoints
            .allocate(FuzzEndpointInfo {
                read_polled,
                read_idx: 0,
            })
            .unwrap()
    }

    pub fn add_pending_client(
        &mut self,
        src_addr: TransportAddress,
        rem_addr: TransportAddress,
        backend: PendingBackend,
    ) -> Rc<SocketId> {
        let client_socket_id = self
            .sockets
            .allocate(SocketState::PendingConnection(PendingSocket {
                local_addr: src_addr,
                rem_addr: rem_addr.clone(),
                backend,
                next_pending: None,
            }))
            .unwrap();

        // Add the client to the pending client chain, if applicable
        match self.socket_locations.get_mut(&rem_addr) {
            None => {
                let polled_id = self.polled_events.allocate(PolledInfo::new()).unwrap();
                self.socket_locations
                    .insert(
                        rem_addr,
                        TransportLocationInfo {
                            reuse_port: false,
                            bound_sockets: Deque::new(),
                            pending: Some(PendingInfo {
                                client: client_socket_id.clone(),
                                poll: polled_id,
                            }),
                        },
                    )
                    .unwrap();
            }
            Some(location_info) => {
                match &location_info.pending {
                    Some(PendingInfo { client, .. }) => {
                        let mut last_client = client.clone();
                        while let Some(SocketState::PendingConnection(PendingSocket {
                            next_pending: Some(id),
                            ..
                        })) = self.sockets.get(&last_client)
                        {
                            last_client = id.clone();
                        }

                        let SocketState::PendingConnection(PendingSocket {
                            next_pending: next_awaiting,
                            ..
                        }) = &mut self.sockets.get_mut(client).unwrap()
                        else {
                            panic!("unexpected internal fizzle state--chain of awaiting clients had invalid socket variant")
                        };

                        *next_awaiting = Some(client_socket_id.clone());
                    }
                    None => {
                        let polled_id = self.polled_events.allocate(PolledInfo::new()).unwrap();
                        location_info.pending = Some(PendingInfo {
                            client: client_socket_id.clone(),
                            poll: polled_id,
                        });
                    }
                }

                if let Some(socket_id) = location_info.bound_sockets.pop_front() {
                    log::debug!("found bound socket at location for pending connection");
                    location_info
                        .bound_sockets
                        .push_back(socket_id.clone())
                        .unwrap();
                    match self.sockets.get(&socket_id).unwrap() {
                        SocketState::Server(server_info) => {
                            log::debug!("notifying server that pending connection exists...");
                            let connect_poll = server_info.ready_to_connect.clone();
                            log::debug!(
                                "connect_poll: {:?}",
                                self.polled_events.get(&connect_poll).unwrap()
                            );
                            self.raise_polled(&connect_poll);
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }

        client_socket_id
    }

    pub fn add_server(&mut self, transport_addr: TransportAddress, backend: ServerBackend) {
        // Create a new polled instance for listeners waiting to accept connections
        let connect_polled_id = self.polled_events.allocate(PolledInfo::new()).unwrap();

        let socket_id = self
            .sockets
            .allocate(SocketState::Server(ServerSocket {
                backend,
                local_addr: transport_addr.clone(),
                connecting: Deque::new(),
                ready_to_connect: connect_polled_id,
            }))
            .unwrap();

        match self.socket_locations.get_mut(&transport_addr) {
            None => {
                let mut bound_sockets = heapless::Deque::new();
                bound_sockets.push_back(socket_id).unwrap();

                self.socket_locations
                    .insert(
                        transport_addr.clone(),
                        TransportLocationInfo {
                            pending: None,
                            reuse_port: false,
                            bound_sockets,
                        },
                    )
                    .unwrap();
            }
            Some(location_info) => {
                debug_assert!(location_info.bound_sockets.is_empty());
                location_info.bound_sockets.push_back(socket_id).unwrap();
            }
        };
    }

    pub fn add_plugin(
        &mut self,
        endpoint: IoEndpointVariant,
        module_id: Rc<PluginId>,
    ) -> Rc<PluginEndpointId> {
        let stream = self.next_stream_id;
        self.next_stream_id = StreamId::from(usize::from(stream) + 1);
        let read_buf = self.buffers.allocate(Buffer::new()).unwrap();
        let read_polled = self.polled_events.allocate(PolledInfo::new()).unwrap();
        let write_buf = self.buffers.allocate(Buffer::new()).unwrap();
        let write_polled = self
            .polled_events
            .allocate(PolledInfo::new_raised())
            .unwrap();

        self.plugins
            .allocate(PluginInfo {
                endpoint,
                stream,
                module_id,
                read_buf,
                read_polled,
                write_buf,
                write_polled,
            })
            .unwrap()
    }

    pub fn ephemeral_address(
        &mut self,
        family: AddressFamily,
        protocol: TransportProtocol,
    ) -> TransportAddress {
        match family {
            AddressFamily::Ipv4 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    // TODO: `panic`s like these won't actually crash the system if they're in subprocesses...
                    // Use a panic handler to kill primary process?
                    panic!("all ephemeral ports were exhausted");
                    // self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                TransportAddress::new_inet(
                    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)),
                    protocol,
                )
            }
            AddressFamily::Ipv6 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                TransportAddress::new_inet(
                    SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
                        port,
                        0,
                        0,
                    )),
                    protocol,
                )
            }
            AddressFamily::Unix => TransportAddress::new_unix(SocketAddrUnix::Unnamed),
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
        self.ready.push_back(ReadyInfo::Worker(worker_id)).unwrap();
    }

    /// This method returns `Ok` if the file was created, and `Err` if a file already
    /// exists at the given path.
    pub fn create_file(&mut self, path: FilePath<MAX_PATH_LEN>) -> Result<Rc<FileId>, Rc<FileId>> {
        match self.file_paths.get(&path) {
            Some(id) => Err(id.clone()),
            None => {
                let buf = self.buffers.allocate(Buffer::new()).unwrap();
                let read_polled = self.polled_events.allocate(PolledInfo::new()).unwrap();
                let write_polled = self
                    .polled_events
                    .allocate(PolledInfo::new_raised())
                    .unwrap();
                let file_id = self
                    .files
                    .allocate(FileBackend::Feedback(StandardFeedback {
                        buf,
                        read_polled,
                        write_polled,
                    }))
                    .unwrap();
                Ok(file_id)
            }
        }
    }
}

/// The unique identifying information for a given thread in a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerId {
    pub process_id: ProcessId,
    pub thread_id: ThreadId,
}

#[derive(Debug)]
pub struct PerRoundClientInfo {
    pub source_address: TransportAddress,
    pub target_address: TransportAddress,
    pub backend: PerRoundClientBackend,
}

#[derive(Clone, Debug)]
pub enum PerRoundClientBackend {
    Fuzz(Rc<FuzzEndpointId>),
    Plugin(Rc<PluginEndpointId>),
}

// TODO: rename...
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadyInfo {
    Poller(Rc<PollerId>),
    Worker(WorkerId),
}

#[derive(Clone, Debug)]
pub enum SignalDestination {
    Process(ProcessId),
    Thread(ThreadId),
}
