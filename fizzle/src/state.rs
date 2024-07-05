pub mod backend;
pub mod comptime;
pub mod fd;
pub mod identifiers;
pub mod plugins;

use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::{Deref, DerefMut};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::{array, env, mem, ptr, slice, thread};

use fizzle_common::io::{AddressFamily, SocketType, TransportAddress, TransportProtocol, UnixAddr, MAX_PATH_LEN};
use fizzle_common::path::{FilePath, SemPath};
use fizzle_common::storage::{Buffer, Rc, KeyedArena};

use fizzle_plugin::{IoEndpointVariant, StreamId};
use heapless::spsc::Queue;

use fxhash::FxBuildHasher;
use heapless::{Deque, FnvIndexMap, FnvIndexSet};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::constants::*;
use crate::semaphore::Semaphore;

use self::backend::{
    ConnectedBackend, ConnectingBackend, ConnectionlessBackend, FileBackend, PendingBackend,
    ServerBackend, StandardFeedback, StdioBackend,
};
use self::fd::{FdInfo, FdResource};
use self::identifiers::*;
use self::plugins::{IoEmulationType, PluginConfigEndpoint, PluginModules};

const THREAD_LOCK_INIT_VALUE: Option<Box<Semaphore>> = None;

pub static FIZZLE_STATE: FizzCell = FizzCell::new();

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = const { RefCell::new(false) };
}

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

/// A global Cell of memory containing all Fizzle state.
///
/// # Safety
///
/// Mutable aliasing within a process is UB in Rust; as such, we use
/// [`thread_locks`](FizzCell::thread_locks) to ensure that only one thread mutably accesses
/// `FizzCell` at a given time.
///
/// Shared mutable memory aliasing across processes isn't technically undefined behavior in Rust, so
/// we don't need them to be separate from the `UnsafeCell` like we do with thread locks.
/// That being said, this data structure aims to provide the guarantee that only one thread
/// of one process is accessing the shared state at any given time when used in tandem with the
/// hooks defined in `src/hooks/pthread.rs` and `src/hooks/proc.rs`.
pub struct FizzCell {
    /// Checked at runtime to ensure that `FizzCell` is not mutably aliased by one process.
    acquired: AtomicBool,
    initialized: AtomicBool,
    reaper_lock: UnsafeCell<MaybeUninit<Semaphore>>,
    /// These are safe as long as the first access
    thread_locks: UnsafeCell<[Option<Box<Semaphore>>; FIZZLE_MAX_THREADS]>,
    // Inter-process locks are held within `FizzState`.
    inner: UnsafeCell<MaybeUninit<FizzState>>,
}

unsafe impl Send for FizzCell {}

unsafe impl Sync for FizzCell {}

// TODO: process locks need to be initialized correctly! They are NOT initialized currently...

impl FizzCell {
    const fn new() -> Self {
        Self {
            acquired: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            reaper_lock: UnsafeCell::new(MaybeUninit::uninit()),
            thread_locks: UnsafeCell::new([THREAD_LOCK_INIT_VALUE; FIZZLE_MAX_THREADS]),
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    #[cold]
    #[inline(never)]
    fn initialize_inner(&self) {
        env_logger::init();
        log::info!("First syscall hooked--`env_logger` initialized.");
        log::trace!("Initializing fizzle state");

        let is_initialized = matches!(env::var(FIZZLE_MEMORY_ENV), Ok(_));

        unsafe {
            // Initialize the reaper lock.
            Semaphore::initialize(&mut *self.reaper_lock.get(), true, 0);
            // Initialize the per-thread semaphore for this thread.
            self.init_thread_lock(&thread::current().id());
            // Initialize FizzState
            (*self.inner.get()).write(FizzState::new());
        }

        log::trace!("Fizzle state initialization complete");

        if !is_initialized { // Main process
            // Now run any applicable processes
            let mut processes = Vec::new();
            comptime::populate_onstartup_processes(&mut processes);

            for mut process in processes {
                let mut ctx = self.acquire();

                // This thread should still be able to execute afterwards
                ctx.mark_thread_ready(thread::current().id());

                let process_id = ctx.global.assign_process_id();
                ctx.global.passthrough_process_id = process_id;

                // TODO: upref all reference-counted global variables here
                // For now we just don't free global variables so it's fine...

                drop(ctx);

                process.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
                process.spawn().unwrap();

                self.pause_current_process();
            }

            let mut onready_processes = Vec::new();
            comptime::populate_onready_processes(&mut onready_processes);
            if onready_processes.is_empty() {
                self.acquire().global.startup_complete = true;
            }
        }
    }

    fn init_thread_lock(&self, thread_id: &ThreadId) {
        unsafe {
            (*self.thread_locks.get())[index_of_thread(thread_id)] = Some(Semaphore::new_boxed(0));
        }
    }

    pub fn get_thread_lock(&self, thread_id: &ThreadId) -> &Semaphore {
        unsafe {
            (*self.thread_locks.get())[index_of_thread(thread_id)]
                .as_deref()
                .unwrap()
        }
    }

    /// Destroys the thread lock of the calling thread.
    fn destroy_thread_lock(&self) {
        unsafe {
            (*self.thread_locks.get())[index_of_thread(&thread::current().id())] = None;
        }
    }

    pub fn init_new_thread(&self) {
        self.acquire()
            .local
            .pthreads
            .insert(unsafe { libc::pthread_self() }, thread::current().id());
        self.init_thread_lock(&thread::current().id());
    }

    pub fn acquire(&self) -> FizzGuard<'_> {
        // TODO: benchmark the perf overhead of this
        if self.acquired.fetch_and(true, Ordering::Relaxed) {
            panic!("internal fizzle error--`FizzCell` global state accessed mutably twice")
        }

        if !self.initialized.fetch_or(true, Ordering::Relaxed) {
            self.initialize_inner()
        }

        FizzGuard { cell: self }
    }

    /// Pauses execution of the current thread and delegates control flow to another thread/process.
    /// Once all threads/processes have finished executing, this returns control flow to the primary
    /// fuzzing process, which signals to the fuzzer that it is ready for the next input.
    pub fn yield_thread(&self) {
        let mut ctx = self.acquire();
        let mut next_worker = None;
        log::trace!("yield_thread internally called");

        // pop PollerId values off `ready_pollers` one at a time
        while let Some(item) = ctx.global.ready.dequeue() {
            match item {
                ReadyInfo::Worker(worker_id) => {
                    log::trace!("Scheduling worker {:?} for execution", worker_id);
                    // new_raised_events will be empty here
                    next_worker = Some(worker_id);
                    break;
                }
                ReadyInfo::Poller(poller_id) => {
                    log::trace!(
                        "Checking if poller {:?} is ready for execution...",
                        poller_id
                    );
                    let global = &mut ctx.global;
                    let poller_info = global.pollers.get(&poller_id).unwrap();
                    for polled_id in poller_info.polled_events.iter() {
                        let polled_info = global.polled_events.get_mut(&polled_id).unwrap();
                        if polled_info.event_raised {
                            next_worker = Some(poller_info.worker_id);
                            log::trace!(
                                "Poller {:?} ready for execution, scheduling worker {:?}",
                                poller_id,
                                poller_info.worker_id
                            );
                            break;
                        }
                    }
                }
            }
        }

        drop(ctx);

        if let Some(worker_id) = next_worker {
            let mut ctx = self.acquire();
            ctx.global.waking_thread_id = Some(worker_id.thread_id);
            let local_process_id = ctx.local.process_id;
            drop(ctx);

            if worker_id.process_id != local_process_id {
                // Invariant: no FizzGuards are being held here
                self.wake_process(worker_id.process_id);
                self.pause_current_process();
            }

            // Now it's this process's turn to execute
            let Some(thread_id) = self.acquire().global.waking_thread_id.take() else {
                panic!("internal fizzle error--no waking_thread_id assigned");
            };

            if thread::current().id() != thread_id {
                log::trace!("Scheduling thread {:?} for execution", thread_id);
                // Invariant: no FizzGuards are being held here
                self.get_thread_lock(&thread_id).post();
                self.pause_current_thread();
            }
            // Now it's this thread's turn to execute
            // TODO: this NEEDS to run in the root process, but we don't check this?

        } else if self::plugins::run_plugins(&mut self.acquire()) {
            // Plugins have queued more workers as ready
            log::trace!(
                "Plugins emitted new input--yielding thread to start next available worker"
            );
            // This shouldn't lead to a stack overflow unless `run_plugins` erroneously
            // returns `true` but doesn't schedule new workers.
            self.yield_thread();

        } else if !self.acquire().global.startup_complete {
            let mut ctx = self.acquire();
            ctx.global.startup_complete = true;

            // Now run any applicable processes
            let mut processes = Vec::new();
            comptime::populate_onready_processes(&mut processes);

            for mut process in processes {
                let mut ctx = self.acquire();

                // This thread should still be able to execute afterwards
                ctx.mark_thread_ready(thread::current().id());

                // TODO: upref all reference-counted global variables here
                // For now we just don't free global variables so it's fine...

                let process_id = ctx.global.assign_process_id();
                ctx.global.passthrough_process_id = process_id;

                drop(ctx);

                process.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
                process.spawn().unwrap();

                self.pause_current_process();
            }
        
            self.yield_thread();
        } else if !self.acquire().global.per_round_endpoints.is_empty() {
            let mut ctx = self.acquire();
            let mut endpoints = FnvIndexSet::new();
            mem::swap(&mut endpoints, &mut ctx.global.per_round_endpoints);

            for socket_id in endpoints.into_iter() {
                let Some(sock_info) = ctx.global.sockets.get_mut(&socket_id) else {
                    continue
                };

                match sock_info {
                    SocketState::PendingConnection(_) => (), // Leave be
                    SocketState::Connected(connected) => {
                        log::debug!("removing connected fuzz/plugin client socket");

                        let target_address = connected.local_addr.clone();
                        let source_address = connected.rem_addr.clone();
                        let client_backend = match &connected.backend {
                            ConnectedBackend::Plugin(plugin_id) => PerRoundClientBackend::Plugin(plugin_id.clone()),
                            ConnectedBackend::Fuzz(fuzz_endpoint_id) => PerRoundClientBackend::Fuzz(fuzz_endpoint_id.clone()),
                            _ => unreachable!(),
                        };

                        if !connected.peer_closed {
                            connected.peer_closed = true;

                            // Now raise all applicable poll events so the reader discovers the peer is closed
                            match connected.backend.clone() {
                                backend::IoBackend::Plugin(plugin_id) => {
                                    let plugin = ctx.global.plugins.get(&plugin_id).unwrap();
                                    let read_polled = plugin.read_polled.clone();
                                    let write_polled = plugin.write_polled.clone();
                                    ctx.raise_polled(&read_polled);
                                    ctx.raise_polled(&write_polled);
                                },
                                backend::IoBackend::Fuzz(fuzz_endpoint_id) => {
                                    let read_polled = ctx.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().read_polled.clone();
                                    ctx.raise_polled(&read_polled);
                                }
                                _ => unreachable!(),
                            }
                        }

                        ctx.global.per_round_clients.push(PerRoundClientInfo {
                            source_address,
                            target_address,
                            backend: client_backend,
                        }).unwrap();
                    }
                    _ => unreachable!(),
                }
            }

            self.yield_thread();

        } else {
            log::trace!("No workers were ready to execute--fuzzing round complete.");
            // No events were triggered for any pollers--move on to next input
            self.fuzz_round_complete();
        }
    }

    /// Notifies the fuzzing engine that the current round of fuzzing has finished.
    /// Note that
    fn fuzz_round_complete(&self) {
        let mut ctx = self.acquire();
        // Communicate that process is finished running

        // A few notes:
        // - deferred forkserver won't work for multi-process fuzzing, full stop.
        // - default forkserver, PCR and Nyx-Net *will* work for multi-process fuzzing, but with caveats:
        //   1. Default forkserver is deterministic but awfully slow (re-instantiates separate processes every time).
        //   2. PCR is fast, but introduces potential instability if state is saved across consecutive connections.
        //   3. Nyx-Net is deterministic and much faster than default forkserver, though harder to set up and has more system overhead.

        // TODO: if using Nyx-Net, handle hypervisor preemption here

        #[cfg(feature = "afl")]
        if !ctx.global.shared_mem_initialized {
            ctx.global.shared_mem_initialized = true;

            #[cfg(feature = "pcr")]
            unsafe {
                crate::__afl_sharedmem_fuzzing = 1;
            }

            log::debug!("calling __afl_manual_init()");
            unsafe {
                crate::__afl_manual_init();
            }
            log::debug!("__afl_manual_init() finished");
        }

        // Wait for input from the fuzzing engine...
        // For AFL++, fuzzing input comes from stdin
        #[cfg(feature = "pcr")]
        unsafe {
            let rounds = if crate::__afl_connected == 0 {
                1
            } else {
                ctx.global.persistent_rounds as libc::c_uint
            };
            if crate::__afl_persistent_loop(rounds) == 0 {
                libc::_exit(0);
            }

            ctx.global.fuzz_input.clear();
            let fuzz_buffer = ctx.global.fuzz_input.remaining_mut();

            if crate::__afl_fuzz_ptr.is_null() {
                let read_amount =
                    libc::read(0, fuzz_buffer.as_mut_ptr() as *mut libc::c_void, 1048576);
                *crate::__afl_fuzz_len = (read_amount & u32::MAX as isize) as u32;
                if read_amount < 0 {
                    panic!("could not read input from stdin")
                }
            } else {
                let afl_buf = slice::from_raw_parts(crate::__afl_fuzz_ptr, *crate::__afl_fuzz_len as usize);
                for (dst, src) in fuzz_buffer.iter_mut().zip(afl_buf.iter()) {
                    dst.write(*src);
                }
            };

            ctx.global
                .fuzz_input
                .did_write(*crate::__afl_fuzz_len as usize);
        }

        #[cfg(not(feature = "pcr"))]
        unsafe {
            let fuzz_buffer = ctx.global.fuzz_input.remaining_mut();
            let read_amount = libc::read(0, fuzz_buffer.as_mut_ptr() as *mut libc::c_void, 1048576);
            if read_amount <= 0 {
                panic!("could not read input from stdin")
            }

            ctx.global.fuzz_input.did_write(read_amount as usize);
        }

        let mut polled_ready = heapless::Vec::<Rc<PolledId>, FIZZLE_MAX_FUZZ_ENDPOINTS>::new();
        for endpoint_info in ctx.global.fuzz_endpoints.values_mut() {
            endpoint_info.read_idx = 0;
            polled_ready
                .push(endpoint_info.read_polled.clone())
                .unwrap();
        }

        log::debug!(
            "{} fuzzing endpoints are marked as ready to fuzz",
            polled_ready.len()
        );

        // Mark appropriate processes/threads as ready to receive input from `fuzz` endpoints
        for polled_id in polled_ready {
            ctx.raise_polled(&polled_id);
        }

        // TODO: inefficient
        let fuzz_input = ctx.global.fuzz_input.clone();

        // TODO: this needs to run in the root process, but we don't check this??
        let modules: Vec<_> = ctx.global.plugins.values().map(|plugin_info| plugin_info.module_id.clone()).collect();
        for module in modules {
            let plugin_module = ctx.local.plugin_modules.as_mut().unwrap().get_mut(&module).unwrap();
            plugin_module.fuzz_round_start(fuzz_input.data());
        }

        let plugin_info_ids: Vec<_> = ctx.global.plugins.values().map(|plugin_info| (plugin_info.read_buf.clone(), plugin_info.write_buf.clone(), plugin_info.read_polled.clone(), plugin_info.write_polled.clone())).collect();

        for (read_buf, write_buf, read_polled, write_polled) in plugin_info_ids {
            ctx.global.buffers.get_mut(&read_buf).unwrap().clear();
            ctx.global.buffers.get_mut(&write_buf).unwrap().clear();
            ctx.lower_polled(&read_polled);
            ctx.raise_polled(&write_polled);
        }

        // Now reload per-round fuzzing clients
        let mut per_round_clients = heapless::Vec::new();
        mem::swap(&mut per_round_clients, &mut ctx.global.per_round_clients);

        for client_info in per_round_clients {
            let socket_id = ctx.global.add_pending_client(client_info.source_address, client_info.target_address, match client_info.backend {
                PerRoundClientBackend::Fuzz(fuzz_endpoint_id) => PendingBackend::Fuzz(fuzz_endpoint_id),
                PerRoundClientBackend::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
            });
            log::debug!("added pending client {:?}", socket_id);
            ctx.global.per_round_endpoints.insert(socket_id).unwrap();
        }

        drop(ctx);

        // If the current running thread isn't ready to receive input, pass on to the next thread.
        self.yield_thread(); // This won't recurse beyond a depth of 2, so long as inputs are passed into the appropriate places here...
    }

    pub fn terminate_thread(&self, term_method: ThreadTermination) -> ! {
        let thread_id = thread::current().id();
        log::info!("thread {:?} being terminated...", thread_id);

        let mut ctx = self.acquire();
        let mut cleanup_routines = ctx
            .local
            .pthread_cleanup
            .remove(&thread_id)
            .unwrap_or_default();

        let pthread_keys: Vec<u32> = ctx.local.pthread_keys.keys().copied().collect();
        for key in pthread_keys {
            if let Some(values) = ctx.local.pthread_key_values.get_mut(&key) {
                if let Some(p) = values.remove(&thread_id) {
                    let mut destructor = *ctx.local.pthread_keys.get(&key).unwrap();
                    destructor.arg = Some(p);
                    cleanup_routines.push_back(destructor);
                }
            }
        }
        drop(ctx);

        set_entered_handler(false);
        for routine in cleanup_routines {
            routine.call();
        }
        set_entered_handler(true);

        let mut ctx = self.acquire();
        // Mark this thread as dead for future threads that may wait on it.
        ctx.local.terminated_threads.insert(thread_id);
        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = ctx.local.awaiting_thread_death.remove(&thread_id) {
            for thread_id in awaiting_threads {
                let process_id = ctx.local.process_id;
                ctx.global
                    .ready
                    .enqueue(ReadyInfo::Worker(WorkerId {
                        process_id,
                        thread_id,
                    }))
                    .unwrap();
            }
        }

        // Delegate execution to another thread via the thread reaper
        if let Some(reaper_id) = ctx.local.reaper {
            drop(ctx);
            // Free this thread's semaphore
            self.destroy_thread_lock();
            FIZZLE_STATE.get_thread_lock(&reaper_id).post()
        } else {
            drop(ctx);
            let handle = std::thread::spawn(move || {
                // Reaper thread
                // The thread that spawned the reaper will immediately wait on its thread lock, so
                // it is safe to `acquire()` global state here.
                let reaper_id = thread::current().id();
                let mut ctx = FIZZLE_STATE.acquire();
                ctx.local.reaper = Some(reaper_id);
                drop(ctx);
                FIZZLE_STATE.init_thread_lock(&reaper_id);
                // Notify thread that initialization has completed
                FIZZLE_STATE.get_thread_lock(&thread_id).post();
                // Await for thread notification before running reaper loop
                FIZZLE_STATE.get_thread_lock(&reaper_id).wait();

                loop {
                    FIZZLE_STATE.yield_thread();
                    // Guaranteed to be listening on the thread-local lock rather than process lock,
                    // as the thread being reaped has to be within the same process as the reaper.
                    // Thus, when the thread being reaped `post()`s, this will return.
                }
            });

            FIZZLE_STATE.get_thread_lock(&thread_id).wait();
            // Free this thread's semaphore
            self.destroy_thread_lock();
            FIZZLE_STATE.get_thread_lock(&handle.thread().id()).post();
        }

        // =======================DANGER ZONE: CONCURRENCY===========================

        // FIZZLE_STATE should not be accessed from this point onwards

        // Now either cancel or signal the current thread to cause it to exit so that threads
        // waiting on `join()` will properly reap threads (and avoid zombies)
        match term_method {
            ThreadTermination::Cancellation => unsafe {
                libc::pthread_cancel(libc::pthread_self());
                libc::sleep(1); // Acts as a backup cancellation point in case `pthread_cancel` didn't work
                panic!("`pthread_cancel` failed to kill current thread");
            },
            ThreadTermination::Exit(retval) => unsafe { libc::pthread_exit(retval) },
            ThreadTermination::SigTerm => unsafe {
                libc::pthread_kill(libc::pthread_self(), libc::SIGTERM);
                libc::sleep(1); // Acts as a backup cancellation point in case `pthread_kill` didn't work
                panic!("`pthread_kill` failed to kill current thread");
            },
        }
    }

    pub fn pause_current_thread(&self) {
        let current_thread = thread::current().id();
        self.get_thread_lock(&current_thread).wait();

        let mut ctx = self.acquire();
        if ctx.local.cancelling_threads.remove(&current_thread) {
            drop(ctx);
            self.terminate_thread(ThreadTermination::Cancellation)
        }
    }

    pub fn pause_current_process(&self) {
        unsafe {
            let ctx = self.acquire();
            ctx.global
                .process_locks[usize::from(ctx.local.process_id)]
                .assume_init_ref()
                .wait();
        }
    }

    fn wake_process(&self, process_id: ProcessId) {
        unsafe {
            self.acquire()
                .global
                .process_locks[usize::from(process_id)]
                .assume_init_ref()
                .post();
        }
    }

    // call this whenever waiting for a single poll event
    pub fn poll_until_ready(&self, polled_id: Rc<PolledId>) {
        let mut ctx = self.acquire();
        if !ctx.polled_is_ready(&polled_id) {
            let poller_id = ctx.new_poller();
            ctx.register_poller(poller_id.clone(), polled_id);
            drop(ctx);
            self.yield_thread();
            self.acquire().delete_poller(poller_id);
        }
    }
}

pub struct FizzGuard<'a> {
    cell: &'a FizzCell,
}

impl Deref for FizzGuard<'_> {
    type Target = FizzState;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.cell.inner.get() as *const FizzState) }
    }
}

impl DerefMut for FizzGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { (*self.cell.inner.get()).assume_init_mut() }
    }
}

impl Drop for FizzGuard<'_> {
    fn drop(&mut self) {
        self.cell.acquired.store(false, Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct FizzState {
    pub local: FizzLocal,
    pub global: &'static mut FizzGlobal,
}

#[no_mangle]
extern "C" fn fizzle_atexit_suspend() {
    loop {
        // TODO: clean up any dangling polling items here, like for `_exit()`/`exit()`
        FIZZLE_STATE.yield_thread()
    }
}

impl FizzState {
    fn new() -> Self {
        // This needs to go before `allocate_global_memory`, as this env variable gets set within it.
        let is_initialized = matches!(env::var(FIZZLE_MEMORY_ENV), Ok(_));

        if env::var(FIZZLE_NOEXIT_ENV).is_ok() {
            unsafe {
                // Registered before any other atexit handler
                // TODO: handle this different with proc interface
                libc::atexit(fizzle_atexit_suspend);
            }
        }

        let global_uninit = Self::allocate_global_memory();
        let global: &'static mut FizzGlobal;
        let process_id: ProcessId;

        match is_initialized {
            true => {
                global = unsafe { global_uninit.assume_init_mut() };
                process_id = global.passthrough_process_id;
                Semaphore::initialize(&mut global.process_locks[usize::from(process_id)], true, 0);

                let transfer_fds = global.transfer_fds.take();
                let local = FizzLocal::new(process_id, transfer_fds, true);
                Self { local, global }
            }
            false => {
                global = FizzGlobal::initialize(global_uninit);
                process_id = ProcessId::from(0);

                let transfer_fds = global.transfer_fds.take();
                let mut local = FizzLocal::new(process_id, transfer_fds, false);

                // Initialize plugins
                let mut endpoints = Vec::new();
                comptime::populate_plugins(&mut endpoints, local.plugin_modules.as_mut().unwrap());
                global.load_config_mappings(endpoints);
                Self { local, global }
            }
        }
    }

    fn allocate_global_memory() -> &'static mut MaybeUninit<FizzGlobal> {
        let size = mem::size_of::<FizzGlobal>();
        let is_multiprocess =
            matches!(env::var(FIZZLE_MULTIPROCESS_ENV), Ok(s) if s.as_str() == "1");

        if !is_multiprocess {
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
                    panic!("failed to mmap")
                }

                return &mut *(location as *mut MaybeUninit<FizzGlobal>);
            }
        }

        let (key, flags) = match env::var(FIZZLE_MEMORY_ENV) {
            Err(_) if !is_multiprocess => {
                log::debug!("allocating IPC_PRIVATE shared memory");
                (
                    libc::IPC_PRIVATE,
                    (libc::S_IRUSR | libc::S_IWUSR) as libc::c_int,
                )
            }
            Ok(var) => {
                log::debug!("attaching to already-created shared memory");
                (
                    var.parse().unwrap(),
                    (libc::S_IRUSR | libc::S_IWUSR) as libc::c_int,
                )
            }
            Err(_) => unsafe {
                let key = libc::getpid();
                env::set_var(FIZZLE_MEMORY_ENV, key.to_string());
                log::debug!("allocating public shared memory object with key {}", key);
                (
                    key,
                    libc::IPC_CREAT
                        | libc::IPC_EXCL
                        | (libc::S_IRUSR | libc::S_IWUSR) as libc::c_int,
                )
            },
        };

        unsafe {
            let shmid = libc::shmget(key, size, flags);
            assert!(
                shmid >= 0,
                "shared memory creation failed (errno {})",
                *libc::__errno_location()
            );

            let location = libc::shmat(shmid, ptr::null_mut(), 0);
            assert!(
                location as isize != -1,
                "mapping shared memory failed (errno {})",
                *libc::__errno_location()
            );

            let ret = libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());
            assert_eq!(
                ret,
                0,
                "failed to make shared memory ephemeral (errno {})",
                *libc::__errno_location()
            );

            &mut *(location as *mut MaybeUninit<FizzGlobal>)
        }
    }

    /* TODO: fix from here onward */

    /// Adds a thread from the current process to the `ready` queue.
    pub fn mark_thread_ready(&mut self, thread_id: ThreadId) {
        let process_id = self.local.process_id;
        self.global
            .ready
            .enqueue(ReadyInfo::Worker(WorkerId {
                process_id,
                thread_id,
            }))
            .unwrap();
    }

    pub fn polled_is_ready(&mut self, polled_id: &Rc<PolledId>) -> bool {
        let polled = self.global.polled_events.get(polled_id).unwrap();
        polled.event_raised
    }

    /// Marks the given polled event as ready.
    ///
    /// If not already raised, this method will enqueue a poller waiting on this polled event
    /// (if such a poller exists).
    pub fn raise_polled(&mut self, polled_id: &Rc<PolledId>) {
        self.global.raise_polled(polled_id);
    }

    // if buffer is empty, then call this
    pub fn lower_polled(&mut self, polled_id: &Rc<PolledId>) {
        self.global
            .polled_events
            .get_mut(polled_id)
            .unwrap()
            .event_raised = false;
    }

    /// Creates a new poller for the currently executing worker.
    pub fn new_poller(&mut self) -> Rc<PollerId> {
        let worker_id = self.current_worker_id();

        self.global
            .pollers
            .allocate(PollerInfo {
                worker_id,
                polled_events: heapless::Vec::new(),
                in_raised_queue: false,
            })
            .unwrap()
    }

    /// Registers `poller_id` as waiting on `polled_id`.
    pub fn register_poller(&mut self, poller_id: Rc<PollerId>, polled_id: Rc<PolledId>) {
        let poller = self.global.pollers.get_mut(&poller_id).unwrap();
        poller.polled_events.push(polled_id.clone()).unwrap();
        let polled = self.global.polled_events.get_mut(&polled_id).unwrap();
        polled.pollers.push(poller_id).unwrap();
    }

    // Ugh. This looks like O(n^2)...
    /// Deletes the given poller, removing any references to it from `Polled` objects.
    pub fn delete_poller(&mut self, poller_id: Rc<PollerId>) {
        let poller = self.global.pollers.get_mut(&poller_id).unwrap();

        if poller.deref().in_raised_queue {
            // TODO: make queue indexable in future
            for _ in 0..self.global.ready.len() {
                let ready = self.global.ready.dequeue().unwrap();
                if let ReadyInfo::Poller(current_poller_id) = &ready {
                    if *current_poller_id != poller_id {
                        self.global.ready.enqueue(ready).unwrap();
                    }
                }
            }
        }

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

    pub fn copy_exec_fds(&mut self) {
        let mut fds = self.local.fds.clone();
        let mut downref_keys  = Vec::new();

        for key in fds.keys() {
            if let Some(FdInfo {
                close_on_exec: true,
                ..
            }) = fds.get(&key) {
                downref_keys.push(key);
            }
        }

        for key in downref_keys {
            fds.downref(&key);
        }

        self.global.transfer_fds = Some(fds);
    }
}

pub struct FizzLocal {
    pub process_id: ProcessId,
    /// Indicates that the thread being awoken should be immediately cancelled and delegate execution back to this thread.
    /// Plugin modules for handling I/O.
    ///
    /// This field is only `Some` in the parent process; all other processes must delegate control
    /// flow to it in order to handle plugin I/O.
    pub plugin_modules: Option<Box<PluginModules>>,
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
    pub pthread_keys: HashMap<libc::pthread_key_t, PThreadRoutine>,
    pub pthread_key_values:
        HashMap<libc::pthread_key_t, HashMap<ThreadId, *mut libc::c_void, FxBuildHasher>>,
    pub futex_waiters: HashMap<*const u32, VecDeque<(u32, ThreadId)>>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    pub cancelling_threads: HashSet<ThreadId, FxBuildHasher>,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath<MAX_PATH_LEN>,
}

impl Debug for FizzLocal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FizzLocal")
            .field("process_id", &self.process_id)
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
            .field("cancelling_threads", &self.cancelling_threads)
            .field("awaiting_thread_death", &self.awaiting_thread_death)
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

impl FizzLocal {
    fn new(
        id: ProcessId,
        transfer_fds: Option<Box<Descriptors>>,
        is_child_process: bool,
    ) -> Self {
        let working_directory =
            FilePath::from_raw_bytes(env::current_dir().unwrap().as_os_str().as_bytes()).unwrap();

        let fds = match transfer_fds {
            Some(fds) => fds,
            None => {
                let mut fds: Box<MaybeUninit<Descriptors>> = Box::new(MaybeUninit::uninit());
                // This needs to remain fixed in a location, so we use a Box with in-place initialization
                unsafe { KeyedArena::initialize(fds.as_mut_ptr()) };

                let mut fds = unsafe { fds.assume_init() };
                fds.allocate_with_key(DescriptorId::from(0), FdInfo::new(FdResource::Stdin)).unwrap();
                fds.allocate_with_key(DescriptorId::from(1), FdInfo::new(FdResource::Stdout)).unwrap();
                fds.allocate_with_key(DescriptorId::from(2), FdInfo::new(FdResource::Stderr)).unwrap();
                fds
            }
        };

        // Insert the current (main) pthread into `pthreads`
        let mut pthreads = HashMap::with_hasher(Default::default());
        pthreads.insert(unsafe { libc::pthread_self() }, thread::current().id());

        let plugin_modules = if is_child_process {
            None
        } else {
            let mut modules: Box<MaybeUninit<PluginModules>> = Box::new(MaybeUninit::uninit());
            // This needs to remain fixed in a location, so we use a Box with in-place initialization
            unsafe { PluginModules::initialize(modules.as_mut_ptr()) };
            Some(unsafe { modules.assume_init() })
        };

        Self {
            process_id: id,
            plugin_modules, 
            reaper: None,
            fds,
            dirs: Default::default(),
            barriers: HashMap::with_hasher(Default::default()),
            condvars: HashMap::with_hasher(Default::default()),
            file_objs: HashMap::with_hasher(Default::default()),
            mutexes: HashMap::with_hasher(Default::default()),
            named_semaphores: HashMap::with_hasher(Default::default()),
            rwlocks: HashMap::with_hasher(Default::default()),
            semaphores: HashMap::with_hasher(Default::default()),
            spinlocks: HashMap::with_hasher(Default::default()),
            pthreads,
            pthread_cleanup: HashMap::with_hasher(Default::default()),
            pthread_keys: HashMap::with_hasher(Default::default()),
            pthread_key_values: HashMap::with_hasher(Default::default()),
            futex_waiters: HashMap::with_hasher(Default::default()),
            terminated_threads: HashSet::with_hasher(Default::default()),
            cancelling_threads: HashSet::with_hasher(Default::default()),
            working_directory,
            awaiting_thread_death: HashMap::with_hasher(Default::default()),
        }
    }
}

#[derive(Debug)]
pub struct FizzGlobal {
    persistent_rounds: usize,
    next_process_id: ProcessId,
    /// The next StreamId available to be assigned to an emulated stream.
    next_stream_id: StreamId,
    /// The next ephemeral port to be assigned to a socket.
    pub next_ephemeral_port: u16,
    /// The thread identifier to be executed by the waking process.
    waking_thread_id: Option<ThreadId>,
    process_locks: [MaybeUninit<Semaphore>; FIZZLE_MAX_PROCESSES],
    transfer_fds: Option<Box<Descriptors>>,
    pub shared_mem_initialized: bool,
    pub passthrough_process_id: ProcessId,
    pub epolls: KeyedArena<EpollId, EpollInfo, FIZZLE_MAX_EPOLLS>,
    pub event_fds: KeyedArena<EventFdId, EventFdInfo, FIZZLE_MAX_EVENTFDS>,
    pub file_paths: FnvIndexMap<FilePath<MAX_PATH_LEN>, Rc<FileId>, FIZZLE_MAX_FILE_PATHS>,
    pub files: KeyedArena<FileId, FileBackend, FIZZLE_MAX_FILES>,
    pub startup_complete: bool,
    pub sem_paths: FnvIndexMap<SemPath, Rc<SemaphoreId>, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub semaphores: KeyedArena<SemaphoreId, SemaphoreInfo, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub pipes: KeyedArena<PipeId, PipeInfo, FIZZLE_MAX_PIPES>,
    pub message_queues: KeyedArena<MessageQueueId, MessageQueueInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
    // TODO: SO_REUSEPORT breaks this...
    pub socket_locations: FnvIndexMap<TransportAddress, SocketLocationInfo, FIZZLE_MAX_SOCKADDRS>,
    pub sockets: KeyedArena<SocketId, SocketState, FIZZLE_MAX_SOCKETS>,
    pub buffers: KeyedArena<BufferId, Buffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub stdio: StdioBackend,
    // Polling infrastructure
    pub plugins: KeyedArena<PluginId, PluginInfo, FIZZLE_MAX_PLUGIN_STREAMS>,
    pub polled_events: KeyedArena<PolledId, PolledInfo, FIZZLE_MAX_POLLED_EVENTS>,
    pub pollers: KeyedArena<PollerId, PollerInfo, FIZZLE_MAX_POLLERS>,
    pub ready: Queue<ReadyInfo, FIZZLE_MAX_QUEUED_READY_POLLERS>,
    pub fuzz_input: Buffer<FIZZLE_MAX_FUZZ_INPUT>,
    pub per_round_clients: heapless::Vec<PerRoundClientInfo, FIZZLE_MAX_PER_ROUND_ENDPOINTS>,
    pub per_round_endpoints: FnvIndexSet<Rc<SocketId>, FIZZLE_MAX_PER_ROUND_ENDPOINTS>,
    pub fuzz_endpoints: KeyedArena<FuzzEndpointId, FuzzEndpointInfo, FIZZLE_MAX_FUZZ_ENDPOINTS>,
    pub prefuzz_rng: rand::rngs::SmallRng,
}

impl FizzGlobal {
    // TODO: initialize() is unsafe--whenever we change the fields in InterprocessState, it becomes
    // unsound until we add the corresponding definition. We should really change it to a trait +
    // proc macro derive.
    /// Takes an uninitialized InterprocessState and initializes it in place.
    fn initialize(state: &mut MaybeUninit<FizzGlobal>) -> &mut FizzGlobal {
        unsafe {
            let state = state.as_mut_ptr();

            *ptr::addr_of_mut!((*state).shared_mem_initialized) = false;
            *ptr::addr_of_mut!((*state).persistent_rounds) = FIZZLE_AFL_LOOP; // TODO: make configurable
            *ptr::addr_of_mut!((*state).next_process_id) = ProcessId::from(1);
            *ptr::addr_of_mut!((*state).next_stream_id) = StreamId::from(0);
            *ptr::addr_of_mut!((*state).next_ephemeral_port) = FIZZLE_EPHEMERAL_PORT_START;
            *ptr::addr_of_mut!((*state).startup_complete) = false;
            *ptr::addr_of_mut!((*state).waking_thread_id) = None;
            *ptr::addr_of_mut!((*state).process_locks) = array::from_fn(|_| MaybeUninit::uninit());
            Semaphore::initialize(&mut (*state).process_locks[0], true, 0);
            *ptr::addr_of_mut!((*state).transfer_fds) = None;
            *ptr::addr_of_mut!((*state).passthrough_process_id) = ProcessId::from(1);
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

            *ptr::addr_of_mut!((*state).stdio) = StdioBackend::Sink;
            KeyedArena::initialize(ptr::addr_of_mut!((*state).plugins));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).polled_events));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).pollers));
            *ptr::addr_of_mut!((*state).ready) = Queue::new();
            *ptr::addr_of_mut!((*state).fuzz_input) = Buffer::new();
            *ptr::addr_of_mut!((*state).per_round_clients) = heapless::Vec::new();
            *ptr::addr_of_mut!((*state).per_round_endpoints) = FnvIndexSet::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).fuzz_endpoints));
            *ptr::addr_of_mut!((*state).prefuzz_rng) =
                SmallRng::seed_from_u64(0xABAD_5EED_ABAD_5EED_u64); // TODO: enable custom seed loading

            &mut (*state)
        }
    }

    fn load_config_mappings(&mut self, endpoints: Vec<PluginConfigEndpoint>) {
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
                            IoEmulationType::Plugin(module_id) => StdioBackend::Plugin(
                                self.add_plugin(endpoint.endpoint_variant.clone(), module_id.clone()),
                            ),
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
                                let backend = FileBackend::Plugin(
                                    self.add_plugin(endpoint.endpoint_variant.clone(), module_id.clone()),
                                );
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
                                let file_id = self.files.allocate(FileBackend::Fuzz(fuzz_endpoint_id)).unwrap();

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

                        self.add_server(TransportAddress::Tcp(addr), backend)
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

                        let target_address = TransportAddress::Tcp(addr);
                        let source_address = self.ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.per_round_clients.push(PerRoundClientInfo {
                                source_address,
                                target_address,
                                backend: match backend {
                                    PendingBackend::Fuzz(fuzz_endpoint_id) => PerRoundClientBackend::Fuzz(fuzz_endpoint_id),
                                    PendingBackend::Plugin(plugin_id) => PerRoundClientBackend::Plugin(plugin_id),
                                    _ => unreachable!(),
                                },
                            }).unwrap();
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

                        self.add_server(TransportAddress::Udp(addr), backend)
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

                        let target_address = TransportAddress::Tcp(addr);
                        let source_address = self.ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.per_round_clients.push(PerRoundClientInfo {
                                source_address,
                                target_address,
                                backend: match backend {
                                    PendingBackend::Fuzz(fuzz_endpoint_id) => PerRoundClientBackend::Fuzz(fuzz_endpoint_id),
                                    PendingBackend::Plugin(plugin_id) => PerRoundClientBackend::Plugin(plugin_id),
                                    _ => unreachable!(),
                                },
                            }).unwrap();
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

                        self.add_server(TransportAddress::Sctp(addr), backend)
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

                        let target_address = TransportAddress::Sctp(addr);
                        let source_address = self.ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.per_round_clients.push(PerRoundClientInfo {
                                source_address,
                                target_address,
                                backend: match backend {
                                    PendingBackend::Fuzz(fuzz_endpoint_id) => PerRoundClientBackend::Fuzz(fuzz_endpoint_id),
                                    PendingBackend::Plugin(plugin_id) => PerRoundClientBackend::Plugin(plugin_id),
                                    _ => unreachable!(),
                                },
                            }).unwrap();
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
    /// If not already raised, this method will enqueue a poller waiting on this polled event
    /// (if such a poller exists).
    fn raise_polled(&mut self, polled_id: &Rc<PolledId>) {
        let polled = self.polled_events.get_mut(polled_id).unwrap();
        if !polled.event_raised {
            polled.event_raised = true;
            let pollers = polled.pollers.clone();
            for poller in pollers {
                if !self.pollers.get(&poller).unwrap().in_raised_queue {
                    self.ready
                        .enqueue(ReadyInfo::Poller(poller))
                        .unwrap();
                }
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
            .allocate(
                FuzzEndpointInfo {
                    read_polled,
                    read_idx: 0,
                },
            )
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
                src_addr,
                rem_addr: rem_addr.clone(),
                backend,
                next_pending: None,
            }))
            .unwrap();

        // Add the client to the pending client chain, if applicable
        match self.socket_locations.get_mut(&rem_addr) {
            None => {
                log::debug!("THE LOCATION INFOR IS GONE SOME");
                let polled_id = self.polled_events.allocate(PolledInfo::new()).unwrap();
                self.socket_locations
                    .insert(
                        rem_addr,
                        SocketLocationInfo {
                            bound_socket: None,
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

                if let Some(socket_id) = location_info.bound_socket.clone() {
                    log::debug!("found bound socket at location for pending connection");
                    match self.sockets.get(&socket_id).unwrap() {
                        SocketState::Server(server_info) => {
                            log::debug!("notifying server that pending connection exists...");
                            let connect_poll = server_info.ready_to_connect.clone();
                            log::debug!("connect_poll: {:?}", self.polled_events.get(&connect_poll).unwrap());
                            self.raise_polled(&connect_poll);
                        },
                        _ => unreachable!()
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
                connecting: Queue::new(),
                ready_to_connect: connect_polled_id,
            }))
            .unwrap();

        match self.socket_locations.get_mut(&transport_addr) {
            None => {
                self.socket_locations
                    .insert(
                        transport_addr.clone(),
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

    pub fn add_plugin(
        &mut self,
        endpoint: IoEndpointVariant,
        module_id: Rc<PluginModuleId>,
    ) -> Rc<PluginId> {
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

    pub fn ephemeral_address(&mut self, family: AddressFamily, protocol: TransportProtocol) -> TransportAddress {
        match family {
            AddressFamily::Ipv4 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                TransportAddress::new_internet(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)), protocol)
            }
            AddressFamily::Ipv6 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                TransportAddress::new_internet(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), port, 0, 0)), protocol)
            }
            AddressFamily::Unix => TransportAddress::Unix(UnixAddr::Unnamed)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadTermination {
    Cancellation,
    Exit(*mut libc::c_void),
    #[allow(unused)]
    SigTerm, // TODO: implement
}

#[derive(Clone, Debug)]
pub struct FuzzEndpointInfo {
    pub read_polled: Rc<PolledId>,
    pub read_idx: usize,
}

#[derive(Clone, Debug)]
pub struct EventFdInfo {
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
    pub is_semaphore: bool,
    pub counter: u64,
}

pub type PThreadDestructor = unsafe extern "C" fn(*mut libc::c_void);

#[derive(Clone, Copy, Debug)]
pub struct PThreadRoutine {
    pub function: PThreadDestructor,
    pub arg: Option<*mut libc::c_void>,
}

impl PThreadRoutine {
    /// Calls the given routine
    pub fn call(self) {
        if let Some(arg) = self.arg {
            unsafe {
                (self.function)(arg);
            }
        }
    }
}

#[derive(Debug)]
pub struct FileObject {
    pub descriptor_id: DescriptorId,
    pub buf: Buffer<FIZZLE_FOPEN_BUFSIZE>,
}

// Each time a Polled is *raised* (i.e., goes from `event_raised: false` to `event_raised: true`),
// the PolledInfo will move all of its `pollers` into the ready queue (if they are not already there).
#[derive(Debug)]
pub struct PolledInfo {
    /// Pollers that this Polled instance is meant to awaken
    pub pollers: heapless::Vec<Rc<PollerId>, FIZZLE_MAX_PER_EVENT_QUEUED_POLLERS>,
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

#[derive(Debug)]
pub struct PerRoundClientInfo {
    source_address: TransportAddress,
    target_address: TransportAddress,
    backend: PerRoundClientBackend,
}

#[derive(Clone, Debug)]
pub enum PerRoundClientBackend {
    Fuzz(Rc<FuzzEndpointId>),
    Plugin(Rc<PluginId>),
}

#[derive(Debug)]
pub struct SocketLocationInfo {
    /// The socket bound to the given location.
    pub bound_socket: Option<Rc<SocketId>>,
    /// Points to an optional linked list of clients that are awaiting this location to exist.
    pub pending: Option<PendingInfo>,
}

#[derive(Clone, Debug)]
pub struct PendingInfo {
    pub client: Rc<SocketId>,
    pub poll: Rc<PolledId>,
}

#[derive(Debug)]
pub struct PollerInfo {
    worker_id: WorkerId,
    polled_events: heapless::Vec<Rc<PolledId>, FIZZLE_MAX_PER_POLLER_QUEUED_EVENTS>,
    in_raised_queue: bool,
}

// TODO: rename...
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadyInfo {
    Poller(Rc<PollerId>),
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
    //    Error state?
}

#[derive(Debug)]
pub struct ConnectionlessSocket {
    pub backend: ConnectionlessBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: Option<TransportAddress>,
}

#[derive(Debug)]
pub struct UnassociatedSocket {
    pub local_addr: Option<TransportAddress>,
    pub family: AddressFamily,
    pub protocol: TransportProtocol,
    pub socktype: SocketType,
}

#[derive(Debug)]
pub struct ServerSocket {
    pub backend: ServerBackend,
    pub local_addr: TransportAddress,
    pub connecting: Queue<Rc<SocketId>, FIZZLE_SOMAXCONN>,
    pub ready_to_connect: Rc<PolledId>,
}

#[derive(Clone, Debug)]
pub struct PendingSocket {
    pub backend: PendingBackend,
    pub next_pending: Option<Rc<SocketId>>,
    pub src_addr: TransportAddress,
    pub rem_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectingSocket {
    pub backend: ConnectingBackend,
    pub connect_polled: Rc<PolledId>,
    pub local_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectedSocket {
    pub backend: ConnectedBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: TransportAddress,
    pub peer_closed: bool,
}

// Runtime active plugin I/O information
#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub endpoint: IoEndpointVariant,
    pub stream: StreamId,
    /// The plugin module to read/write from.
    pub module_id: Rc<PluginModuleId>,
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_buf: Rc<BufferId>,
    pub write_polled: Rc<PolledId>,
}

#[derive(Debug)]
pub struct EpollInfo {
    pub interests: FnvIndexMap<DescriptorId, EpollInterest, FIZZLE_MAX_EPOLL_FDS>,
}

#[derive(Clone, Debug)]
pub struct EpollInterest {
    pub direction: EpollDirection,
    pub user_data: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpollDirection {
    None,
    Read(PolledStatus),
    Write(PolledStatus),
    Both(PolledStatus, PolledStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolledStatus {
    Pollable(Rc<PolledId>),
    /// The file descriptor was invalid.
    BadFd,
    /// The requested object will never return polled output (such as attempting to read `stdout`).
    NotPollable,
    /// The requested object will immediately return polled output (such as writing to `stderr`).
    ImmediatelyPollable,
}

type Descriptors = KeyedArena<DescriptorId, FdInfo, FIZZLE_MAX_FDS>;

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
    pub peer: Option<Rc<PipeId>>,
    /// The buffer this pipe reads in data from.
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
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

impl Default for RwLockInfo {
    fn default() -> Self {
        Self {
            state: RwLockState::Available,
            awaiting_read: VecDeque::new(),
            awaiting_write: VecDeque::new(),
            holding_state: HashSet::with_hasher(Default::default()),
        }
    }
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

// ====== Helper Functions ======

fn index_of_thread(thread: &ThreadId) -> usize {
    let mut hasher = ThreadHasher::new();
    thread.hash(&mut hasher);
    hasher.finish() as usize
}
