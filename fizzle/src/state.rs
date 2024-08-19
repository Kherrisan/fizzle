use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::{Deref, DerefMut};
use std::os::unix::ffi::OsStrExt;
use std::thread::ThreadId;
use std::{array, env, mem, ptr, thread};

use bitflags::bitflags;
use fizzle_common::io::{AddressFamily, SocketAddrUnix, TransportAddress, TransportProtocol, MAX_PATH_LEN};
use fizzle_common::path::{FilePath, SemPath};
use fizzle_common::storage::Buffer;
use fizzle_plugin::{IoEndpointVariant, StreamId};
use fxhash::FxBuildHasher;
use heapless::{Deque, FnvIndexMap, FnvIndexSet};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::{comptime, state};
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
use crate::handlers::plugin::{PluginId, PluginInfo};
use crate::handlers::plugin_module::PluginModuleId;
use crate::handlers::polled::{PolledId, PolledInfo};
use crate::handlers::poller::{PollerId, PollerInfo};
use crate::handlers::process::ProcessId;
use crate::handlers::rwlock::{RwLockInfo, RwLockPtr};
use crate::handlers::semaphore::{SemaphoreId, SemaphoreInfo, SemaphorePtr};
use crate::handlers::socket::{PendingInfo, PendingSocket, ServerSocket, SocketId, SocketState, TransportLocationInfo};
use crate::handlers::spinlock::SpinlockPtr;
use crate::handlers::thread::{PThreadRoutine, ThreadTermination};
use crate::once::NcOnceCell;
use crate::plugins::{IoEmulationType, PluginConfigEndpoint, PluginModules};
use crate::semaphore::Semaphore;

use crate::backend::{
    FileBackend, PendingBackend, ServerBackend, StandardFeedback, StdioBackend
};

pub use private::FizzleSingleton;

mod private {
    pub struct FizzleSingleton {
        /// Empty private field to ensure `FizzleSingleton` isn't constructed outside of
        /// `fizzle_state_singleton()`.
        _private: (),
    }

    impl FizzleSingleton {
        pub(super) fn new() -> Self {
            let mut singleton = FizzleSingleton {
                _private: (),
            };

            singleton.post_init();
            singleton
        }
    }
}

static FIZZLE_STATE: NcOnceCell<RefCell<FizzleState>> = NcOnceCell::new();

static THREAD_LOCKS: NcOnceCell<[RefCell<Option<Semaphore>>; FIZZLE_MAX_THREADS]> = NcOnceCell::new();

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
pub unsafe fn fizzle_state_singleton() -> FizzleSingleton {
    FizzleSingleton::new()
}

impl FizzleSingleton {
    pub fn init() -> RefCell<FizzleState> {
        env_logger::init();
        log::info!("First syscall hooked--initializing Fizzle state...");

        let ctx = RefCell::new(FizzleState::new());
        let mut state = ctx.borrow_mut();
        let process_id = state.local.process_id;
        let sem_opt = &mut state.global.process_locks[usize::from(process_id)];

        // TODO: this seems to behave safely, but check with Miri
        unsafe {
            let uninit_sem = (*(ptr::from_mut(sem_opt) as *mut Option<MaybeUninit<Semaphore>>)).insert(MaybeUninit::uninit());
            Semaphore::initialize(uninit_sem, true, 0);
        }

        drop(state);

        log::info!("Fizzle state initialization complete.");

        ctx
    }

    pub fn post_init(&mut self) {
        let mut state = self.acquire(); // Runs init()
        if state.local.post_init_done {
            return
        }
        state.local.post_init_done = true;

        log::trace!("running post-initialization.");

        if state.local.process_id.is_main_process() {
            // Spawn the plugin handler (MUST run in main process)
            let thread_id = thread::current().id();
            drop(state);
            thread::spawn(move || {
                set_entered_handler(true);
                let mut ctx = unsafe { fizzle_state_singleton() };
                let plugin_thread_id = thread::current().id();
                // The thread that spawned the reaper will immediately wait on its thread lock, so
                // it is safe to `acquire()` global state here.
                let mut state = ctx.acquire();

                let plugin_worker = WorkerId {
                    process_id: state.local.process_id,
                    thread_id: plugin_thread_id,
                };

                state.global.plugin_worker = Some(plugin_worker);

                drop(state);

                ctx.init_thread_lock(&plugin_thread_id);
                // Notify thread that initialization has completed
                ctx.thread_lock(&thread_id).as_ref().unwrap().post();

                // Wait for plugin worker to be delegated execution
                ctx.thread_lock(&plugin_thread_id).as_ref().unwrap().wait();
                
                loop {
                    crate::handlers::plugin::handle_plugins(&mut ctx);
                    ctx.yield_thread();
                }
            });

            self.thread_lock(&thread::current().id()).as_ref().unwrap().wait();
            // Plugin process initialization is complete
            
            // Now run any applicable processes
            let mut processes = Vec::new();
            comptime::populate_onstartup_processes(&mut processes);

            for mut process in processes {
                let mut state = self.acquire();

                // This thread should still be able to execute afterwards
                state.mark_thread_delayed_ready(thread::current().id());

                let process_id = state.global.assign_process_id();
                state.global.passthrough_process_id = process_id;

                // TODO: upref all reference-counted global variables here
                // For now we just don't free global variables so it's fine...

                // TODO: put this in a method on its own
                process.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
                process.env(FIZZLE_MEMORY_ENV, std::env::var(FIZZLE_MEMORY_ENV).unwrap());
                process.spawn().unwrap();

                log::debug!("waiting on process_id {:?}", state.local.process_id);

                state.global
                    .process_locks[usize::from(state.local.process_id)]
                    .as_ref()
                    .unwrap()
                    .wait();

                drop(state);
            }

            let mut onready_processes = Vec::new();
            comptime::populate_onready_processes(&mut onready_processes);
            if onready_processes.is_empty() {
                let mut state = self.acquire();
                state.global.startup_complete = true;
                drop(state);
            }
        }
    }

    /// Acquires the global shared state for mutable access.
    pub fn acquire(&mut self) -> RefMut<'_, FizzleState> {
        FIZZLE_STATE.get_or_init(Self::init).borrow_mut()       
    }

    pub fn run_outside_shim<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce() -> R
    {
        debug_assert!(state::has_entered_handler());
        state::set_entered_handler(false);
        let ret = f();
        state::set_entered_handler(true);
        ret
    }

    pub fn thread_lock(&mut self, thread_id: &ThreadId) -> Ref<'_, Option<Semaphore>> {
        let locks = THREAD_LOCKS.get_or_situate(Self::thread_locks_situate);

        let thread_idx = crate::handlers::thread::index_of_thread(thread_id);
        locks[thread_idx].borrow()
    }

    fn thread_locks_situate(uninit: &mut MaybeUninit<[RefCell<Option<Semaphore>>; FIZZLE_MAX_THREADS]>) -> &mut [RefCell<Option<Semaphore>>; FIZZLE_MAX_THREADS] {
        let locks = uninit.write(array::from_fn(|_| RefCell::new(None)));

        // Now initialize the current thread's lock.
        let current_thread_idx = crate::handlers::thread::index_of_thread(&thread::current().id());
        let mut sem_opt = locks[current_thread_idx].borrow_mut();
        let sem_opt_deref = sem_opt.deref_mut();

        // TODO: this seems to behave safely, but check with Miri
        unsafe {
            let uninit_sem = (*(ptr::from_mut(sem_opt_deref) as *mut Option<MaybeUninit<Semaphore>>)).insert(MaybeUninit::uninit());
            Semaphore::initialize(uninit_sem, false, 0);
        }
        drop(sem_opt);

        locks
    }

    fn init_thread_lock(&mut self, thread_id: &ThreadId) {
        let locks = THREAD_LOCKS.get_or_situate(Self::thread_locks_situate);
        let thread_idx = crate::handlers::thread::index_of_thread(thread_id);

        let mut sem_opt = locks[thread_idx].borrow_mut();
        let sem_opt_deref = sem_opt.deref_mut();

        // TODO: this seems to behave safely, but check with Miri
        unsafe {
            let uninit_sem = (*(ptr::from_mut(sem_opt_deref) as *mut Option<MaybeUninit<Semaphore>>)).insert(MaybeUninit::uninit());
            Semaphore::initialize(uninit_sem, false, 0);
        }
        drop(sem_opt);
    }

    /// Destroys the thread lock of the calling thread.
    fn destroy_thread_lock(&mut self) {
        let locks = THREAD_LOCKS.get_or_situate(Self::thread_locks_situate);
        let thread_idx = crate::handlers::thread::index_of_thread(&thread::current().id());
        *locks[thread_idx].borrow_mut() = None;
    }

    pub fn init_new_thread(&mut self) {
        let mut state = self.acquire();
        state.local
            .pthreads
            .insert(unsafe { libc::pthread_self() }, thread::current().id());
        state.local.pthread_sigmasks.insert(thread::current().id(), SignalSet::empty());
        drop(state);
        self.init_thread_lock(&thread::current().id());
    }

    // TODO: yield_thread has become ugly, long and incomprehensible. Refactor.

    /// Pauses execution of the current thread and delegates control flow to another thread/process.
    /// Once all threads/processes have finished executing, this returns control flow to the primary
    /// fuzzing process, which signals to the fuzzer that it is ready for the next input.
    pub fn yield_thread(&mut self) {
        let mut state = self.acquire();
        let mut next_worker = None;
        log::trace!("yield_thread internally called");

        // pop PollerId values off `ready_pollers` one at a time
        while let Some(item) = state.global.ready.pop_front() {
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
                    let global = &mut state.global;
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

        if let Some(worker_id) = next_worker {
            state.global.waking_thread_id = Some(worker_id.thread_id);
            let local_process_id = state.local.process_id;
            drop(state);

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
                self.thread_lock(&thread_id).as_ref().unwrap().post();
                self.pause_current_thread();
            }

        } else {
            let plugin_worker = state.global.plugin_worker.unwrap();

            // Schedule the plugin worker for execution
            state.global.mark_worker_ready(plugin_worker);
            drop(state);

            // Yield to the plugin worker
            self.yield_thread();
        }
    }

    // TODO: refactor `fuzz_round_complete` as well

    /// Notifies the fuzzing engine that the current round of fuzzing has finished.
    /// Note that
    pub fn fuzz_round_complete(&mut self) {
        let mut state = self.acquire();
        // Communicate that process is finished running

        // A few notes:
        // - deferred forkserver won't work for multi-process fuzzing, full stop.
        // - default forkserver, PCR and Nyx-Net *will* work for multi-process fuzzing, but with caveats:
        //   1. Default forkserver is deterministic but awfully slow (re-instantiates separate processes every time).
        //   2. PCR is fast, but introduces potential instability if state is saved across consecutive connections.
        //   3. Nyx-Net is deterministic and much faster than default forkserver, though harder to set up and has more system overhead.

        // TODO: if using Nyx-Net, handle hypervisor preemption here

        #[cfg(feature = "afl")]
        if !state.global.shared_mem_initialized {
            state.global.shared_mem_initialized = true;

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
                state.global.persistent_rounds as libc::c_uint
            };
            if crate::__afl_persistent_loop(rounds) == 0 {
                libc::_exit(0);
            }

            state.global.fuzz_input.clear();
            let fuzz_buffer = state.global.fuzz_input.remaining_mut();

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

            state.global
                .fuzz_input
                .did_write(*crate::__afl_fuzz_len as usize);
        }

        #[cfg(not(feature = "pcr"))]
        unsafe {
            let fuzz_buffer = state.global.fuzz_input.remaining_mut();
            let read_amount = libc::read(0, fuzz_buffer.as_mut_ptr() as *mut libc::c_void, 1048576);
            if read_amount <= 0 {
                panic!("could not read input from stdin")
            }

            state.global.fuzz_input.did_write(read_amount as usize);
        }

        let mut polled_ready = heapless::Vec::<Rc<PolledId>, FIZZLE_MAX_FUZZ_ENDPOINTS>::new();
        for endpoint_info in state.global.fuzz_endpoints.values_mut() {
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
            state.raise_polled(&polled_id);
        }

        // TODO: inefficient
        let fuzz_input = state.global.fuzz_input.clone();

        let modules: Vec<_> = state.global.plugins.values().map(|plugin_info| plugin_info.module_id.clone()).collect();
        for module in modules {
            let plugin_module = state.local.plugin_modules.as_mut().unwrap().get_mut(&module).unwrap();
            plugin_module.fuzz_round_start(fuzz_input.data());
        }

        let plugin_info_ids: Vec<_> = state.global.plugins.values().map(|plugin_info| (plugin_info.read_buf.clone(), plugin_info.write_buf.clone(), plugin_info.read_polled.clone(), plugin_info.write_polled.clone())).collect();

        for (read_buf, write_buf, read_polled, write_polled) in plugin_info_ids {
            state.global.buffers.get_mut(&read_buf).unwrap().clear();
            state.global.buffers.get_mut(&write_buf).unwrap().clear();
            state.lower_polled(&read_polled);
            state.raise_polled(&write_polled);
        }

        // Now reload per-round fuzzing clients
        let mut per_round_clients = heapless::Vec::new();
        mem::swap(&mut per_round_clients, &mut state.global.per_round_clients);

        log::info!("{} per-round clients to be initialized...", per_round_clients.len());

        for client_info in per_round_clients {
            let socket_id = state.global.add_pending_client(client_info.source_address, client_info.target_address, match client_info.backend {
                PerRoundClientBackend::Fuzz(fuzz_endpoint_id) => PendingBackend::Fuzz(fuzz_endpoint_id),
                PerRoundClientBackend::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
            });
            log::debug!("added pending client {:?}", socket_id);
            state.global.per_round_endpoints.insert(socket_id).unwrap();
        }

        drop(state);
    }

    pub fn terminate_thread(&mut self, term_method: ThreadTermination) -> ! {
        let thread_id = thread::current().id();
        log::info!("thread {:?} being terminated...", thread_id);

        let mut state = self.acquire();
        let mut cleanup_routines =state 
            .local
            .pthread_cleanup
            .remove(&thread_id)
            .unwrap_or_default();

        let pthread_keys: Vec<u32> = state.local.pthread_keys.keys().copied().collect();
        for key in pthread_keys {
            if let Some(values) = state.local.pthread_key_values.get_mut(&key) {
                if let Some(p) = values.remove(&thread_id) {
                    let mut destructor = *state.local.pthread_keys.get(&key).unwrap();
                    destructor.arg = Some(p);
                    cleanup_routines.push_back(destructor);
                }
            }
        }
        drop(state);

        self.run_outside_shim(|| {
            for routine in cleanup_routines {
                routine.call();
            }
        });

        let mut state = self.acquire();

        // Mark this thread as dead for future threads that may wait on it.
        state.local.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = state.local.awaiting_thread_death.remove(&thread_id) {
            let process_id = state.local.process_id;
            for thread_id in awaiting_threads {
                state.global
                    .ready
                    .push_back(ReadyInfo::Worker(WorkerId {
                        process_id,
                        thread_id,
                    }))
                    .unwrap();
            }
        }

        // Clean up local state of thread
        state.local.pthread_cleanup.remove(&thread::current().id());
        state.local.pthread_sigmasks.remove(&thread::current().id());
        state.local.pthreads.remove(&unsafe { libc::pthread_self() });

        // Delegate execution to another thread via the thread reaper
        if let Some(reaper_id) = state.local.reaper {
            drop(state);
            // Free this thread's semaphore
            self.destroy_thread_lock();
            self.thread_lock(&reaper_id).as_ref().unwrap().post()
        } else {
            drop(state);
            let handle = std::thread::spawn(move || {
                set_entered_handler(true);
                let mut ctx = unsafe { fizzle_state_singleton() };
                // Reaper thread
                // The thread that spawned the reaper will immediately wait on its thread lock, so
                // it is safe to `acquire()` global state here.
                let reaper_id = thread::current().id();
                let mut state = ctx.acquire();
                state.local.reaper = Some(reaper_id);
                drop(state);
                ctx.init_thread_lock(&reaper_id);
                // Notify thread that initialization has completed
                ctx.thread_lock(&thread_id).as_ref().unwrap().post();
                // Await for thread notification before running reaper loop
                ctx.thread_lock(&reaper_id).as_ref().unwrap().wait();

                loop {
                    ctx.yield_thread();
                    // Guaranteed to be listening on the thread-local lock rather than process lock,
                    // as the thread being reaped has to be within the same process as the reaper.
                    // Thus, when the thread being reaped `post()`s, this will return.
                }
            });

            self.thread_lock(&thread_id).as_ref().unwrap().wait();
            // Free this thread's semaphore
            self.destroy_thread_lock();
            self.thread_lock(&handle.thread().id()).as_ref().unwrap().post();
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

    pub fn pause_current_thread(&mut self) {
        let current_thread = thread::current().id();
        self.thread_lock(&current_thread).as_ref().unwrap().wait();

        let mut state = self.acquire();
        if state.local.cancelling_threads.remove(&current_thread) {
            drop(state);
            self.terminate_thread(ThreadTermination::Cancellation)
        }
    }

    pub fn pause_current_process(&mut self) {
        let state = self.acquire();
        state.global
            .process_locks[usize::from(state.local.process_id)]
            .as_ref()
            .unwrap()
            .wait();
    }

    fn wake_process(&mut self, process_id: ProcessId) {
        self.acquire()
            .global
            .process_locks[usize::from(process_id)]
            .as_ref()
            .unwrap()
            .post();
    }

    // call this whenever waiting for a single poll event
    pub fn poll_until_ready(&mut self, polled_id: Rc<PolledId>) {
        let mut state = self.acquire();
        if !state.polled_is_ready(&polled_id) {
            let poller_id = state.new_poller();
            state.register_poller(poller_id.clone(), polled_id);
            drop(state);
            self.yield_thread();
            self.acquire().delete_poller(poller_id);
        }
    }
}

#[derive(Debug)]
pub struct FizzleState {
    pub local: ProcessLocalState,
    pub global: &'static mut InterprocessState,
}

impl FizzleState {
    fn new() -> Self {
        // This needs to go before `allocate_global_memory`, as this env variable gets set within it.
        let is_initialized = matches!(env::var(FIZZLE_MEMORY_ENV), Ok(_));

        let global_uninit = Self::allocate_global_memory();
        let global: &'static mut InterprocessState;
        let process_id: ProcessId;

        match is_initialized {
            true => {
                global = unsafe { global_uninit.assume_init_mut() };
                process_id = global.passthrough_process_id;

                let transfer_fds = global.transfer_fds.take();
                let local = ProcessLocalState::new(process_id, transfer_fds, true);
                Self { local, global }
            }
            false => {
                global = InterprocessState::initialize(global_uninit);
                process_id = ProcessId::from(0);

                let transfer_fds = global.transfer_fds.take();
                let mut local = ProcessLocalState::new(process_id, transfer_fds, false);

                // Initialize plugins
                let mut endpoints = Vec::new();
                comptime::populate_plugins(&mut endpoints, local.plugin_modules.as_mut().unwrap());
                global.load_config_mappings(endpoints);
                Self { local, global }
            }
        }
    }

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
                    panic!("failed to mmap global memory (errno {})", *libc::__errno_location())
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
                (
                    var.parse().unwrap(),
                    (libc::S_IRUSR | libc::S_IWUSR) as i32
                )
            }
            Err(_) => unsafe {
                let key = libc::getpid();
                env::set_var(FIZZLE_MEMORY_ENV, key.to_string());
                log::debug!("allocating public shared memory object with key {}", key);
                (
                    key,
                    (libc::S_IRUSR | libc::S_IWUSR) as i32
                    | libc::IPC_CREAT | libc::IPC_EXCL
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

            /*
            let ret = libc::shmctl(shmid, libc::IPC_RMID, ptr::null_mut());
            assert_eq!(
                ret,
                0,
                "failed to make shared memory ephemeral (errno {})",
                *libc::__errno_location()
            );
            */

            &mut *(location as *mut MaybeUninit<InterprocessState>)
        }
    }

    /// Adds a thread from the current process to the `ready` queue.
    pub fn mark_thread_ready(&mut self, thread_id: ThreadId) {
        let process_id = self.local.process_id;
        self.global
            .ready
            .push_back(ReadyInfo::Worker(WorkerId {
                process_id,
                thread_id,
            }))
            .unwrap();
    }

    pub fn mark_thread_delayed_ready(&mut self, thread_id: ThreadId) {
        let process_id = self.local.process_id;
        self.global
            .delayed_ready
            .push_back(ReadyInfo::Worker(WorkerId {
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
    /// If not already raised, this method will push_back a poller waiting on this polled event
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
                let ready = self.global.ready.pop_front().unwrap();
                if let ReadyInfo::Poller(current_poller_id) = &ready {
                    if *current_poller_id != poller_id {
                        self.global.ready.push_back(ready).unwrap();
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
            if let Some(DescriptorInfo {
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

pub struct ProcessLocalState {
    pub post_init_done: bool,
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
    pub pthread_keys: HashMap<libc::pthread_key_t, PThreadRoutine, FxBuildHasher>,
    pub pthread_key_values:
        HashMap<libc::pthread_key_t, HashMap<ThreadId, *mut libc::c_void, FxBuildHasher>, FxBuildHasher>,
    pub pthread_sigmasks: HashMap<ThreadId, SignalSet, FxBuildHasher>,
    pub futex_waiters: HashMap<*const u32, VecDeque<(u32, ThreadId)>, FxBuildHasher>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    pub cancelling_threads: HashSet<ThreadId, FxBuildHasher>,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath<MAX_PATH_LEN>,
}

impl Debug for ProcessLocalState {
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

impl ProcessLocalState {
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
                fds.allocate_with_key(DescriptorId::from_raw_fd(0), DescriptorInfo::new(FdResource::Stdin)).unwrap();
                fds.allocate_with_key(DescriptorId::from_raw_fd(1), DescriptorInfo::new(FdResource::Stdout)).unwrap();
                fds.allocate_with_key(DescriptorId::from_raw_fd(2), DescriptorInfo::new(FdResource::Stderr)).unwrap();
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
            post_init_done: false,
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
            pthread_sigmasks: HashMap::with_hasher(Default::default()),
            futex_waiters: HashMap::with_hasher(Default::default()),
            terminated_threads: HashSet::with_hasher(Default::default()),
            cancelling_threads: HashSet::with_hasher(Default::default()),
            working_directory,
            awaiting_thread_death: HashMap::with_hasher(Default::default()),
        }
    }
}

#[derive(Debug)]
pub struct InterprocessState {
    pub plugin_worker: Option<WorkerId>,
    pub persistent_rounds: usize,
    pub next_process_id: ProcessId,
    /// The next StreamId available to be assigned to an emulated stream.
    pub next_stream_id: StreamId,
    /// The next ephemeral port to be assigned to a socket.
    pub next_ephemeral_port: u16,
    /// The thread identifier to be executed by the waking process.
    pub waking_thread_id: Option<ThreadId>,
    pub process_locks: [Option<Semaphore>; FIZZLE_MAX_PROCESSES],
    pub process_sigmasks: KeyedArena<ProcessId, SignalInfo, FIZZLE_MAX_PROCESSES>,
    pub pids: FnvIndexMap<libc::pid_t, ProcessId, FIZZLE_MAX_PROCESSES>,
    pub transfer_fds: Option<Box<Descriptors>>,
    pub shared_mem_initialized: bool,
    pub passthrough_process_id: ProcessId,
    pub epolls: KeyedArena<EpollId, EpollInfo, FIZZLE_MAX_EPOLLS>,
    pub event_fds: KeyedArena<EventfdId, EventfdInfo, FIZZLE_MAX_EVENTFDS>,
    pub file_paths: FnvIndexMap<FilePath<MAX_PATH_LEN>, Rc<FileId>, FIZZLE_MAX_FILE_PATHS>,
    pub files: KeyedArena<FileId, FileBackend, FIZZLE_MAX_FILES>,
    pub startup_complete: bool,
    pub sem_paths: FnvIndexMap<SemPath, Rc<SemaphoreId>, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub semaphores: KeyedArena<SemaphoreId, SemaphoreInfo, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub pipes: KeyedArena<PipeId, PipeInfo, FIZZLE_MAX_PIPES>,
    pub message_queues: KeyedArena<MessageQueueId, MessageQueueInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
    // TODO: SO_REUSEPORT breaks this...
    pub socket_locations: FnvIndexMap<TransportAddress, TransportLocationInfo, FIZZLE_MAX_SOCKADDRS>,
    pub sockets: KeyedArena<SocketId, SocketState, FIZZLE_MAX_SOCKETS>,
    pub buffers: KeyedArena<BufferId, Buffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub stdio: StdioBackend,
    // Polling infrastructure
    pub plugins: KeyedArena<PluginId, PluginInfo, FIZZLE_MAX_PLUGIN_STREAMS>,
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
    // TODO: initialize() is unsafe--whenever we change the fields in InterprocessState, it becomes
    // unsound until we add the corresponding definition. We should really change it to a trait +
    // proc macro derive.
    /// Takes an uninitialized InterprocessState and initializes it in place.
    fn initialize(state: &mut MaybeUninit<InterprocessState>) -> &mut InterprocessState {
        unsafe {
            let state = state.as_mut_ptr();
            *ptr::addr_of_mut!((*state).plugin_worker) = None;
            *ptr::addr_of_mut!((*state).shared_mem_initialized) = false;
            *ptr::addr_of_mut!((*state).persistent_rounds) = FIZZLE_AFL_LOOP; // TODO: make configurable
            *ptr::addr_of_mut!((*state).next_process_id) = ProcessId::from(1);
            *ptr::addr_of_mut!((*state).next_stream_id) = StreamId::from(0);
            *ptr::addr_of_mut!((*state).next_ephemeral_port) = FIZZLE_EPHEMERAL_PORT_START;
            *ptr::addr_of_mut!((*state).startup_complete) = false;
            *ptr::addr_of_mut!((*state).waking_thread_id) = None;
            *ptr::addr_of_mut!((*state).process_locks) = array::from_fn(|_| None);
            KeyedArena::initialize(ptr::addr_of_mut!((*state).process_sigmasks));
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

            *ptr::addr_of_mut!((*state).stdio) = StdioBackend::Passthrough;
            KeyedArena::initialize(ptr::addr_of_mut!((*state).plugins));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).polled_events));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).pollers));
            *ptr::addr_of_mut!((*state).ready) = Deque::new();
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

                        self.add_server(TransportAddress::new_inet(addr, TransportProtocol::Tcp), backend)
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

                        let target_address = TransportAddress::new_inet(addr, TransportProtocol::Tcp);
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

                        self.add_server(TransportAddress::new_inet(addr, TransportProtocol::Udp), backend)
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

                        let target_address = TransportAddress::new_inet(addr, TransportProtocol::Udp);
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

                        self.add_server(TransportAddress::new_inet(addr, TransportProtocol::Sctp), backend)
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

                        let target_address = TransportAddress::new_inet(addr, TransportProtocol::Sctp);
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
    /// If not already raised, this method will push_back a poller waiting on this polled event
    /// (if such a poller exists).
    fn raise_polled(&mut self, polled_id: &Rc<PolledId>) {
        let polled = self.polled_events.get_mut(polled_id).unwrap();
        if !polled.event_raised {
            polled.event_raised = true;
            let pollers = polled.pollers.clone();
            for poller in pollers {
                if !self.pollers.get(&poller).unwrap().in_raised_queue {
                    self.ready
                        .push_back(ReadyInfo::Poller(poller))
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
                    location_info.bound_sockets.push_back(socket_id.clone()).unwrap();
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
                assert!(location_info.bound_sockets.is_empty());
                location_info.bound_sockets.push_back(socket_id).unwrap();
            }
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
                TransportAddress::new_inet(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)), protocol)
            }
            AddressFamily::Ipv6 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                TransportAddress::new_inet(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), port, 0, 0)), protocol)
            }
            AddressFamily::Unix => TransportAddress::new_unix(SocketAddrUnix::Unnamed)
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
    Plugin(Rc<PluginId>),
}

// TODO: rename...
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadyInfo {
    Poller(Rc<PollerId>),
    Worker(WorkerId),
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct SignalSet: u64 {
        const SIGHUP = 1 << 0;
        const SIGINT = 1 << 1;
        const SIGQUIT = 1 << 2;
        const SIGILL = 1 << 3;
        const SIGTRAP = 1 << 4;
        const SIGABRT = 1 << 5;
        const SIGIOT = 1 << 5;
        const SIGBUS = 1 << 6;
        const SIGFPE = 1 << 7;
        const SIGKILL = 1 << 8;
        const SIGUSR1 = 1 << 9;
        const SIGSEGV = 1 << 10;
        const SIGUSR2 = 1 << 11;
        const SIGPIPE = 1 << 12;
        const SIGALRM = 1 << 13;
        const SIGTERM = 1 << 14;
        const SIGSTKFLT = 1 << 15;
        const SIGCHLD = 1 << 16;
        const SIGCONT = 1 << 17;
        const SIGSTOP = 1 << 18;
        const SIGTSTP = 1 << 19;
        const SIGTTIN = 1 << 20;
        const SIGTTOU = 1 << 21;
        const SIGURG = 1 << 22;
        const SIGXCPU = 1 << 23;
        const SIGXFSZ = 1 << 24;
        const SIGVTALRM = 1 << 25;
        const SIGGPROF = 1 << 26;
        const SIGWINCH = 1 << 27;
        const SIGIO = 1 << 28;
        const SIGPOLL = 1 << 28;
        const SIGLOST = 1 << 28;
        const SIGPWR = 1 << 29;
        const SIGSYS = 1 << 30;
        const SIGUNUSED = 1 << 30;
        const SIGRTMIN = 1 << 31;
    }
}


type SigAction = fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void);

#[derive(Clone, Debug)]
pub struct SignalInfo {
    mask: SignalSet,
    raised: SignalSet,
    handlers: [Option<SigAction>; 32],
}

impl SignalInfo {
    pub fn new() -> Self {
        // TODO: initialize from existing signals       

        Self {
            mask: SignalSet::empty(),
            raised: SignalSet::empty(),
            handlers: array::from_fn(|_| None),
        }
    }
}

type Descriptors = KeyedArena<DescriptorId, DescriptorInfo, FIZZLE_MAX_FDS>;
