use std::os::fd::RawFd;
use std::process::Command;
use std::thread::ThreadId;
use std::time::Duration;
use std::{mem, ptr, thread};

use heapless::FnvIndexSet;

use crate::arena::Rc;
use crate::backend::{ConnectedBackend, PendingBackend};
use crate::constants::{FIZZLE_MAX_FUZZ_ENDPOINTS, FIZZLE_MEMORY_ENV};
use crate::handlers::file::CowId;
use crate::handlers::id::{WorkerId, WorkerInfo};
use crate::handlers::mutex::MutexStatus;
use crate::handlers::polled::PolledId;
use crate::handlers::process::*;
use crate::handlers::signal::*;
use crate::handlers::socket::SocketState;
use crate::plugins;
use crate::state::{self, PerRoundClientBackend, PerRoundClientInfo, SignalDestination};
use crate::state::{FizzleSingleton, FizzleState, ReadyInfo};

// Input parameters are contained within the event
pub trait Event {
    /// The Success type associated with the event.
    type Success;
    /// The Error type associated with the event.
    type Error;

    /// Executes the action associated with the event.
    ///
    /// This function is meant to be called repeatedly until one of `Outcome::Success` or
    /// `Outcome::Error` is returned. It should not be called following that.
    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error>;
}

pub enum Outcome<S, E> {
    /// The value S should be returned for the hook function.
    Success(S),
    /// The error value and errno specified in E should be returned for the function.
    Error(E),
    /// Yields the current thread and executes the next ready worker.
    Yield(Option<Duration>),
    /// The event should move on to its next action immediately.
    Continue,
    /// Yields the current thread without executing the next ready worker.
    Pause(DelegationSource),
    /// Terminates the given thread's execution.
    TerminateThread(TerminationMethod),
    /// Terminates the given process's execution.
    TerminateProcess(TerminationMethod),
    /// Executes the given method.
    // TODO: make this generic in the future.
    Execute(unsafe extern "C" fn()),
    /// Send the given signal to the specified worker
    SendSignal(SignalDestination, RaisedSignalInfo),
    /// Create a copy-on-write (CoW) file with contents taken from the (optional) file descriptor.
    CreateCow(Option<RawFd>),
    /// Migrate the CoW file descriptor to the calling process.
    MigrateCow(CowId),
}

pub struct Scheduler;

impl Scheduler {
    pub fn handle_event<T: Event>(
        ctx: &mut FizzleSingleton,
        mut event: T,
    ) -> Result<T::Success, T::Error> {
        loop {
            // First `acquire()` call for state allocates and instantiates shared memory
            let mut state = ctx.acquire();

            // Initialize local/global state if needed
            if !state.local.is_initialized {
                state.initialize_state();

                if let Some(main_state) = state.local.main_state.as_mut() {
                    let mut startup_commands = Vec::new();
                    mem::swap(&mut startup_commands, &mut main_state.onstartup_commands);
                    drop(state);

                    while let Some(onstartup) = startup_commands.pop() {
                        Scheduler::run_subprocess(ctx, onstartup);
                    }
                } else {
                    drop(state);
                }
            } else {
                drop(state);
            }

            // pre-actions here

            let mut state = ctx.acquire();

            match event.run(&mut state) {
                Outcome::Success(s) => return Ok(s),
                Outcome::Error(e) => return Err(e),
                Outcome::Continue => (),
                Outcome::Yield(None) => {
                    log::debug!("Thread being yielded");
                    drop(state);
                    Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);
                }
                Outcome::Yield(Some(Duration::ZERO)) => (), // Same as Continue
                Outcome::Yield(Some(duration)) => {
                    log::debug!("Thread being yielded with timeout");

                    if duration.as_millis() <= 1000 {
                        // TODO: make into constant
                        // Short enough
                        drop(state);
                        ctx.acquire().mark_thread_ready(thread::current().id());

                        Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);
                    } else if duration.as_millis() <= 5000 {
                        // TODO: make into constant
                        // Long, but not so long as to time out the fuzzer
                        drop(state);
                        ctx.acquire()
                            .mark_thread_delayed_ready(thread::current().id());

                        Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);
                    } else {
                        // Long enough to consider as a permanent timer--just leave
                    }
                }
                Outcome::Pause(src) => {
                    // SAFETY: `state` is never used prior to being dropped, so noalias isn't violated
                    drop(state);
                    Scheduler::yield_worker(ctx, DelegationAction::PauseCurrentWorker(src));
                }
                Outcome::TerminateThread(method) => {
                    drop(state);
                    Scheduler::terminate_thread(ctx, method);
                }
                Outcome::TerminateProcess(method) => {
                    drop(state);
                    Scheduler::terminate_process(ctx, method)
                }
                Outcome::Execute(f) => {
                    drop(state);
                    Scheduler::run_outside_hook(ctx, || unsafe { f() });
                }
                Outcome::SendSignal(destination, raised_info) => {
                    drop(state);
                    Scheduler::send_signal(ctx, destination, raised_info);
                }
            }
        }
    }

    /// Waits for the specified poll event to become available.
    fn poll_until_ready(ctx: &mut FizzleSingleton, polled_id: Rc<PolledId>) {
        let mut state = ctx.acquire();
        if !state.polled_is_ready(&polled_id) {
            let poller_id = state.new_poller();
            state.register_poller(poller_id.clone(), polled_id);
            drop(state);
            Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);
            ctx.acquire().delete_poller(poller_id);
        }
    }

    /// Gives up execution of the current thread until it is rescheduled.
    ///
    /// This should be the **only** method that uses per-thread/process semaphores.
    fn yield_worker(ctx: &mut FizzleSingleton, action: DelegationAction) {
        // SAFETY: `state` must not be accessed prior to 'yielded
        let current_thread_id = thread::current().id();

        let mut delegation_state = DelegationState::from(action);
        let mut delegation_source: DelegationSource;

        'yielded: loop {
            // 1. Perform a delegation action
            match delegation_state {
                // The current worker is creating a new thread
                DelegationState::PauseCurrentWorker(src) => delegation_source = src,
                // The current worker is done being yielded
                DelegationState::RunCurrentWorker => return,
                // The current worker is delegating execution to whatever is available
                DelegationState::RunNextWorker => {
                    let mut state = ctx.acquire();

                    let Some(worker_id) = Self::next_ready_worker(&mut state) else {
                        delegation_state = DelegationState::NoMoreWorkers;
                        continue 'yielded;
                    };

                    log::debug!("Scheduling worker {:?} for execution", worker_id);

                    // Give the next process the info it needs to run the correct thread
                    state.global.waking_id = Some(worker_id.thread_id);
                    let local_process_id = state.local.process_id;
                    drop(state);

                    if worker_id.process_id != local_process_id {
                        // Execution needs to move to another process
                        Scheduler::wake_process(ctx, worker_id.process_id);
                        delegation_source = DelegationSource::Process;
                    } else if worker_id.thread_id != current_thread_id {
                        // Execution needs to move to another thread
                        ctx.thread_lock(&worker_id.thread_id)
                            .as_ref()
                            .unwrap()
                            .post();
                        delegation_source = DelegationSource::Thread;
                    } else {
                        let mut state = ctx.acquire();
                        state.global.waking_id = None;
                        drop(state);

                        delegation_state = DelegationState::RunCurrentWorker;
                        continue 'yielded;
                    }
                }
                DelegationState::RunProcess(process_id) => {
                    // Immediately awaken the specified process (used during cancellation)
                    delegation_source = DelegationSource::Process;
                    Scheduler::wake_process(ctx, process_id);
                }
                DelegationState::RunThread(thread_id) => {
                    // Immediately awaken the specified thread (used during cancellation)
                    delegation_source = DelegationSource::Thread;
                    ctx.thread_lock(&thread_id).as_ref().unwrap().post();
                }
                DelegationState::NoMoreWorkers => {
                    log::debug!("No workers were ready for execution");

                    let state = ctx.acquire();
                    let local_id = state.local.process_id;
                    let main_id = ProcessId::main_process();
                    drop(state);

                    // No more workers means it's time for plugins to execute
                    if local_id != main_id {
                        // Execution needs to be moved to the main process
                        Scheduler::wake_process(ctx, main_id);
                        delegation_source = DelegationSource::Process;
                    } else {
                        // Execution is already in the main process
                        delegation_state = DelegationState::RunPlugins;
                        continue 'yielded;
                    }
                }
                DelegationState::RunPlugins => {
                    let mut state = ctx.acquire();
                    assert!(state.local.process_id.is_main_process());

                    if plugins::run_plugins(&mut state) {
                        // There are outstanding inputs from plugins to be processed
                        delegation_state = DelegationState::RunNextWorker;
                        continue 'yielded;
                    } else if let Some(ready) = state.global.delayed_ready.pop_front() {
                        // There are outstanding delayed workers
                        state.global.ready.push_back(ready).unwrap();
                        delegation_state = DelegationState::RunNextWorker;
                        continue 'yielded;
                    } else if let Some(onready) = state
                        .local
                        .main_state
                        .as_mut()
                        .unwrap()
                        .onready_commands
                        .pop()
                    {
                        // Not all `onready` subprocesses have been spawned

                        drop(state);
                        Scheduler::run_subprocess(ctx, onready);
                        delegation_source = DelegationSource::Process;
                    } else if !state.global.per_round_endpoints.is_empty() {
                        // Not all endpoints have been disconnected for this round?

                        drop(state);
                        Scheduler::remove_perround_endpoints(ctx);
                        delegation_state = DelegationState::RunNextWorker;
                        continue 'yielded;
                    } else {
                        drop(state);

                        // Everything is ready for the next round now
                        Scheduler::round_complete(ctx);

                        // TODO: handle per_round_endpoints here??

                        delegation_state = DelegationState::RunNextWorker;
                        continue 'yielded;
                    }
                }
                DelegationState::TerminateThread(method) => {
                    Scheduler::terminate_thread(ctx, method);
                }
                DelegationState::SignalToThread(worker_id, signal) => {
                    let mut state = ctx.acquire();

                    let thread_id = state.global.ids.get(&worker_id).unwrap().thread_id;

                    if thread_id != thread::current().id() {
                        state.global.signal = Some((SignalDestination::Thread(worker_id), signal));
                        delegation_source = DelegationSource::Thread;
                        drop(state);
                        ctx.thread_lock(&thread_id).as_ref().unwrap().post();
                    } else {
                        delegation_state = DelegationState::HandleSignal(signal);
                        continue 'yielded;
                    }
                }
                DelegationState::HandleSignal(raised_info) => {
                    Scheduler::handle_signal(ctx, raised_info);
                    delegation_state = DelegationState::RunNextWorker;
                    continue 'yielded;
                }
            }

            // SAFETY: no mutable references to `state` being held here

            // 2. Suspend execution
            Scheduler::await_delegation(ctx, delegation_source);

            // 3. Determine next delegation action based on global state
            // NOTE: delegation_state SHOULD NOT be relied on here.

            // It may seem like a message channel of sorts would be better here than the semaphore
            // + shared memory fields that are currently used. The big issue is that threads of
            // one process are unaware of threads of another process (since thread info is in
            // process-local state), so there's no clean way to execute this message channel
            // between two threads in separate processes.

            // This process/thread has been awakened...
            let mut state = ctx.acquire();
            delegation_state = if let Some(thread_id) = state.local.cancelling.take() {
                // ...because it was cancelled via `pthread_cancel()`

                assert_eq!(thread_id, thread::current().id());
                DelegationState::TerminateThread(TerminationMethod::Cancellation)
            } else if let Some((dst, raised_info)) = state.global.signal.take() {
                // ...because it has received a signal (e.g. from `kill()`, `pthread_kill()`)
                let signum = raised_info.signum();

                match dst {
                    SignalDestination::Process(worker_id) => {
                        // The worker raising the signal should always yield to the appropriate process
                        // assert_eq!(process_id, state.local.process_id);

                        // Assign one of the threads of this process to receive the signal
                        let mut chosen_thread = None;
                        for (thread_id, siginfo) in state.local.signals.iter_mut() {
                            if !siginfo.blocked.contains(SignalSet::from_signum(signum)) {
                                chosen_thread = Some(*thread_id);
                            }
                        }

                        if let Some(chosen_thread) = chosen_thread {
                            let tid = *state.local.tids.get(&chosen_thread).unwrap();
                            // Now run that thread
                            DelegationState::SignalToThread(tid, raised_info)
                        } else {
                            // None of the threads were ready--assign the (blocked) signal to one of the threads
                            'assigned: {
                                for siginfo in state.local.signals.values_mut() {
                                    if siginfo.raised[signum as usize].is_none() {
                                        siginfo.raised[signum as usize] = Some(raised_info);
                                        break 'assigned DelegationState::RunNextWorker;
                                    }
                                }

                                log::warn!(
                                    "Signal {} for PID {:?} was dropped",
                                    raised_info.signum(),
                                    worker_id,
                                );
                                DelegationState::RunNextWorker
                            }
                        }
                    }
                    SignalDestination::Thread(worker_id) => {
                        let worker_info = *state.global.ids.get(&worker_id).unwrap();
                        // The worker raising the signal should always yield to the appropriate process
                        debug_assert_eq!(worker_info.process_id, state.local.process_id);

                        let siginfo = state.local.signals.get_mut(&worker_info.thread_id).unwrap();

                        if siginfo.blocked.contains(SignalSet::from_signum(signum)) {
                            // The signal was blocked--store it (if there's room)
                            if siginfo.raised[signum as usize].is_none() {
                                siginfo.raised[signum as usize] = Some(raised_info);
                            } else {
                                log::warn!(
                                    "Signal {} for TID {:?} was dropped",
                                    raised_info.signum(),
                                    worker_id,
                                );
                            }

                            DelegationState::RunNextWorker
                        } else if worker_info.thread_id != thread::current().id() {
                            DelegationState::SignalToThread(worker_id, raised_info)
                        } else {
                            DelegationState::HandleSignal(raised_info)
                        }
                    }
                }
            } else if let Some(_worker_id) = state.global.exiting_id.take() {
                // ...because a thread is being reaped and needs to delegate execution

                // TODO: use `pthread_join` or `waitpid` here to ensure completion?
                DelegationState::RunNextWorker
            } else if let Some(thread_id) = state.global.waking_id.take() {
                // ...because a worker in this process is being actively scheduled

                if thread_id == thread::current().id() {
                    // This worker is being actively scheduled
                    DelegationState::RunCurrentWorker
                } else {
                    // Some other thread is being scheduled--delegate execution
                    state.global.waking_id = Some(thread_id);
                    DelegationState::RunThread(thread_id)
                }
            } else {
                // The thread was awoken despite no event...
                unreachable!()
            }
        }
    }

    // TODO: clean this up better
    fn round_complete(ctx: &mut FizzleSingleton) {
        let mut state = ctx.acquire();

        Scheduler::prepare_fuzz_input(&mut state);

        // Reset fuzz endpoint state (e.g. endpoints configured with the `fuzz` option)
        let mut polled_ready = heapless::Vec::<_, FIZZLE_MAX_FUZZ_ENDPOINTS>::new();
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

        // Seed each plugin with the fuzzing input
        let modules: Vec<_> = state
            .global
            .plugins
            .values()
            .map(|plugin_info| plugin_info.module_id.clone())
            .collect();
        for module in modules {
            let (local, global) = state.split();
            let plugin_module = local
                .main_state
                .as_mut()
                .unwrap()
                .plugins
                .get_mut(&module)
                .unwrap();
            plugin_module.fuzz_round_start(global.fuzz_input.data());
        }

        // Gather all plugin endpoints
        let plugin_info_ids: Vec<_> = state
            .global
            .plugins
            .values()
            .map(|plugin_info| {
                (
                    plugin_info.read_buf.clone(),
                    plugin_info.write_buf.clone(),
                    plugin_info.read_polled.clone(),
                    plugin_info.write_polled.clone(),
                )
            })
            .collect();

        // Reset plugin endpoint state
        for (read_buf, write_buf, read_polled, write_polled) in plugin_info_ids {
            state.global.buffers.get_mut(&read_buf).unwrap().clear();
            state.global.buffers.get_mut(&write_buf).unwrap().clear();
            state.lower_polled(&read_polled);
            state.raise_polled(&write_polled);
        }

        // Reset per-round fuzzing clients
        let mut per_round_clients = heapless::Vec::new();
        mem::swap(&mut per_round_clients, &mut state.global.per_round_clients);

        log::info!(
            "{} per-round clients to be initialized...",
            per_round_clients.len()
        );

        // Add new pending per-round fuzzing clients
        for client_info in per_round_clients {
            let socket_id = state.global.add_pending_client(
                client_info.source_address,
                client_info.target_address,
                match client_info.backend {
                    PerRoundClientBackend::Fuzz(fuzz_endpoint_id) => {
                        PendingBackend::Fuzz(fuzz_endpoint_id)
                    }
                    PerRoundClientBackend::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
                },
            );
            log::debug!("added pending client {:?}", socket_id);
            state.global.per_round_endpoints.insert(socket_id).unwrap();
        }

        drop(state);
    }

    fn remove_perround_endpoints(ctx: &mut FizzleSingleton) {
        let mut state = ctx.acquire();

        let mut endpoints = FnvIndexSet::new();
        mem::swap(&mut endpoints, &mut state.global.per_round_endpoints);

        for socket_id in endpoints.into_iter() {
            let Some(sock_info) = state.global.sockets.get_mut(&socket_id) else {
                continue;
            };

            let local_transport = sock_info.local_transport();

            match &mut sock_info.state {
                SocketState::PendingConnection(_) => (), // Leave be
                SocketState::Connected(connected) => {
                    log::debug!("removing connected fuzz/plugin client socket");

                    let target_address = local_transport.unwrap();

                    let source_address = connected.rem_addr.clone();
                    let client_backend = match &connected.backend {
                        ConnectedBackend::Plugin(plugin_id) => {
                            PerRoundClientBackend::Plugin(plugin_id.clone())
                        }
                        ConnectedBackend::Fuzz(fuzz_endpoint_id) => {
                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id.clone())
                        }
                        _ => unreachable!(),
                    };

                    if !connected.peer_closed {
                        connected.peer_closed = true;

                        // Now raise all applicable poll events so the reader discovers the peer is closed
                        match connected.backend.clone() {
                            ConnectedBackend::Plugin(plugin_id) => {
                                let plugin = state.global.plugins.get(&plugin_id).unwrap();
                                let read_polled = plugin.read_polled.clone();
                                let write_polled = plugin.write_polled.clone();
                                state.raise_polled(&read_polled);
                                state.raise_polled(&write_polled);
                            }
                            ConnectedBackend::Fuzz(fuzz_endpoint_id) => {
                                let read_polled = state
                                    .global
                                    .fuzz_endpoints
                                    .get(&fuzz_endpoint_id)
                                    .unwrap()
                                    .read_polled
                                    .clone();
                                state.raise_polled(&read_polled);
                            }
                            _ => unreachable!(),
                        }
                    }

                    state
                        .global
                        .per_round_clients
                        .push(PerRoundClientInfo {
                            source_address,
                            target_address,
                            backend: client_backend,
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
        }
    }

    fn prepare_fuzz_input(state: &mut FizzleState) {
        #[cfg(feature = "afl")]
        if !state.global.shared_mem_initialized {
            state.global.shared_mem_initialized = true;

            unsafe {
                #[cfg(feature = "pcr")]
                crate::__afl_sharedmem_fuzzing = 1;

                crate::__afl_manual_init();
            }
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
                libc::_exit(0); // _exit to avoid `atexit` handlers that would reduce efficiency
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
                let afl_buf =
                    slice::from_raw_parts(crate::__afl_fuzz_ptr, *crate::__afl_fuzz_len as usize);
                for (dst, src) in fuzz_buffer.iter_mut().zip(afl_buf.iter()) {
                    dst.write(*src);
                }
            };

            state
                .global
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
    }

    fn send_cancellation(ctx: &mut FizzleSingleton, target: libc::pthread_t) {
        let mut state = ctx.acquire();
        let thread_id = state.local.pthreads.get(&target).unwrap().id;
        assert!(state.local.cancelling.replace(thread_id).is_none());

        // Continue this worker once the cancellation is complete
        let this_worker = state.current_worker_id();
        state
            .global
            .ready
            .push_front(ReadyInfo::Worker(this_worker));
        drop(state);

        Scheduler::yield_worker(ctx, DelegationAction::RunThread(thread_id));
    }

    fn send_signal(
        ctx: &mut FizzleSingleton,
        dst: SignalDestination,
        raised_info: RaisedSignalInfo,
    ) {
        let signum = raised_info.signum();

        // Save the current worker
        let mut state = ctx.acquire();
        let current_worker = state.current_worker_id();
        let worker_id = match dst {
            SignalDestination::Process(p) => p,
            SignalDestination::Thread(p) => p,
        };

        let dst_process = state.global.ids.get(&worker_id).unwrap().process_id;
        let dst_thread = state.global.ids.get(&worker_id).unwrap().thread_id;

        let disposition = &state
            .global
            .processes
            .get(&dst_process)
            .unwrap()
            .signal_handlers[signum as usize];
        if let SigDisposition::Ignore = disposition {
            return; // Ignores the signal without saving it
        }

        // Once the signal has been received and handled, keep running the worker that sent it
        state
            .global
            .ready
            .push_front(ReadyInfo::Worker(current_worker));

        // Add the signal to the global state
        assert!(state.global.signal.replace((dst, raised_info)).is_none());
        drop(state);

        // TODO: delegate to process/thread signal is being sent to
        if dst_process == current_worker.process_id {
            Scheduler::yield_worker(ctx, DelegationAction::RunThread(dst_thread))
        } else {
            Scheduler::yield_worker(ctx, DelegationAction::RunProcess(dst_process))
        }
    }

    fn handle_signal(ctx: &mut FizzleSingleton, raised_info: RaisedSignalInfo) {
        let mut state = ctx.acquire();
        let (local, global) = state.split();
        let thread_id = thread::current().id();
        let process_id = local.process_id;

        let signum = raised_info.signum();
        let thread_siginfo = local.signals.get_mut(&thread_id).unwrap();
        let proc_siginfo = global.processes.get_mut(&process_id).unwrap();

        match (
            &proc_siginfo.signal_handlers[signum as usize],
            thread_siginfo
                .blocked
                .contains(SignalSet::from_signum(signum)),
        ) {
            (_, true) => {
                if thread_siginfo.raised[signum as usize].is_some() {
                    // If there is already a pending signal, the incoming one is dropped
                    log::warn!(
                        "Signal {} for {:?}, {:?} dropped",
                        signum,
                        process_id,
                        thread_id
                    );
                } else {
                    thread_siginfo.raised[signum as usize].insert(raised_info);
                    if thread_siginfo
                        .sigwait_set
                        .contains(SignalSet::from_signum(signum))
                    {
                        // A sigwait signal has become pending--awaken the waiting thread
                        thread_siginfo.sigwait_set = SignalSet::empty();
                        state.mark_thread_ready(thread_id);
                    }
                }
            }
            (SigDisposition::Default, false) => {
                if thread_siginfo.sigsuspend {
                    // Any call to `sigsuspend()` should return for this process
                    thread_siginfo.sigsuspend = false;
                    thread_siginfo.interrupted = true;
                    state.mark_thread_ready(thread_id);
                }

                match SignalSet::from_signum(signum) {
                    // Ignore the signal--do nothing
                    SignalSet::SIGCHLD | SignalSet::SIGURG | SignalSet::SIGWINCH => (),
                    // Unpause the process
                    SignalSet::SIGCONT => {
                        unimplemented!("`SIGCONT` signal");
                    }
                    // Stop the thread (process?)
                    SignalSet::SIGSTOP
                    | SignalSet::SIGTSTP
                    | SignalSet::SIGTTIN
                    | SignalSet::SIGTTOU => {
                        unimplemented!("`SIGSTOP` family of signals");
                    }
                    _ => {
                        drop(state);
                        Scheduler::terminate_process(ctx, TerminationMethod::Signal(raised_info))
                    }
                }
            }
            (SigDisposition::Handler(handler), false) => {
                let handler = *handler;

                if thread_siginfo.sigsuspend {
                    // Any call to `sigsuspend()` should return for this process
                    thread_siginfo.sigsuspend = false;
                    state.mark_thread_ready(thread_id);
                }

                drop(state);
                Scheduler::run_outside_hook(ctx, || unsafe {
                    handler(raised_info.signum());
                });
            }
            (SigDisposition::Action(action), false) => {
                let action = *action;

                if thread_siginfo.sigsuspend {
                    // Any call to `sigsuspend()` should return for this process
                    thread_siginfo.sigsuspend = false;
                    state.mark_thread_ready(thread_id);
                }

                drop(state);

                let mut siginfo = siginfo_t::from_raised(raised_info);
                Scheduler::run_outside_hook(ctx, || unsafe {
                    action(
                        raised_info.signum(),
                        ptr::addr_of_mut!(siginfo),
                        ptr::null_mut(),
                    );
                });
            }
            (SigDisposition::Ignore, _) => unreachable!(), // This should be handled in `send_signal()`
        }
    }

    /// Causes a thread with the specified ThreadId to be awakened.
    fn wake_thread(ctx: &mut FizzleSingleton, thread_id: &ThreadId) {
        ctx.thread_lock(thread_id).as_ref().unwrap().post();
    }

    /// Causes a thread in the process with the specified ID to be awakened.
    fn wake_process(ctx: &mut FizzleSingleton, process_id: ProcessId) {
        ctx.acquire().global.process_locks[usize::from(process_id)]
            .as_ref()
            .unwrap()
            .post();
    }

    /// Executes the provided command as a child process within the Fizzle harness.
    fn run_subprocess(_ctx: &mut FizzleSingleton, mut cmd: Command) {
        // TODO: need to upref all reference-counted global variables here

        // TODO: need to prepare signal handlers to be passed

        cmd.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
        cmd.env(FIZZLE_MEMORY_ENV, std::env::var(FIZZLE_MEMORY_ENV).unwrap());
        cmd.spawn().unwrap();
    }

    /// Fetches the next available worker with a completed task from Fizzle's state.
    ///
    /// If no workers are ready, this function will return `None`.
    fn next_ready_worker(state: &mut FizzleState) -> Option<WorkerInfo> {
        while let Some(item) = state.global.ready.pop_front() {
            match item {
                ReadyInfo::Worker(worker_id) => return Some(worker_id),
                ReadyInfo::Poller(poller_id) => {
                    log::trace!(
                        "Checking if poller {:?} is ready for execution...",
                        poller_id
                    );
                    let global = &mut state.global;

                    let poller_info = global.pollers.get_mut(&poller_id).unwrap();

                    for polled_id in poller_info.raised_events.iter() {
                        let polled_info = global.polled_events.get_mut(&polled_id).unwrap();
                        if polled_info.event_raised {
                            log::trace!("Poller {:?} is ready for execution", poller_id);
                            return Some(poller_info.worker_id);
                        }
                    }

                    log::trace!(
                        "Poller {:?} is not ready for execution--clearing events",
                        poller_id
                    );
                    poller_info.raised_events.clear();
                }
            }
        }

        return None;
    }

    /// Suspends execution of the current thread and awaits delegation from the specified source.
    fn await_delegation(ctx: &mut FizzleSingleton, source: DelegationSource) {
        let thread_id = thread::current().id();

        match source {
            DelegationSource::Process => {
                let state = ctx.acquire();
                // SAFETY: The below action holds a mutable reference to `state` while delegating
                // execution to another worker. **HOWEVER**, this is _not_ undefined behavior, for the
                // following reasons:
                //
                // 1. This method passes off execution to another _process_, not another _thread_. Once
                // other processes are done, Fizzle guarantees execution will be handed back to **this**
                // thread in this process, not some other thread in this process. Thus, this process never
                // holds two mutable references to `state` at once; rather, it and some other process hold
                // one mutable reference each to shared memory. The rust compiler cannot possibly reason
                // about the presence or absense of other processes accessing data in its memory model, so
                // this _technically_ upholds the "no mutable aliasing" requirement.
                //
                // 2. Okay, 1. addresses mutable aliasing but it doesn't consider the more fundamental issue
                // that mutable aliasing is trying to address: data races. Once again, Fizzle is safe in
                // this regard--since this process **only** accesses a semaphore within the state and waits
                // on that semaphore until another process signals that it can continue on, any interprocess
                // memory will be synchronized by the time this process begins execution again.
                //
                // To sum up, mutable aliasing doesn't count since we're reasoning across multiple processes,
                // and data races won't happen because we've made sure our ducks (**sic** semaphores) are all
                // in a row.
                state.global.process_locks[usize::from(state.local.process_id)]
                    .as_ref()
                    .unwrap()
                    .wait();
            }
            DelegationSource::Thread => {
                ctx.thread_lock(&thread_id).as_ref().unwrap().wait();
            }
        }
    }

    /// Terminates the current thread, cleaning up its resources along the way.
    fn terminate_thread(ctx: &mut FizzleSingleton, method: TerminationMethod) -> ! {
        log::info!("Thread being terminated...");

        let thread_id = thread::current().id();
        let mut state = ctx.acquire();

        let mut cleanup_routines = state
            .local
            .pthread_cleanup
            .remove(&thread_id)
            .unwrap_or_default();

        // Remove and gather cleanup routines for pthread keys
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

        // Run cleanup routines, hooking any functions within the routines
        Self::run_outside_hook(ctx, || {
            for routine in cleanup_routines {
                routine.call();
            }
        });

        // Clean up this thread's semaphore
        ctx.destroy_thread_lock();

        let mut state = ctx.acquire();

        // Mark this thread as dead for future threads that may wait on it.
        state.local.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = state.local.awaiting_thread_death.remove(&thread_id) {
            let process_id = state.local.process_id;
            for thread_id in awaiting_threads {
                state.global.mark_worker_ready(WorkerInfo {
                    process_id,
                    thread_id,
                });
            }
        }

        // Clean up local state of thread
        state.local.pthread_cleanup.remove(&thread_id);
        state.local.signals.remove(&thread_id);
        let thread_info = state
            .local
            .pthreads
            .remove(&unsafe { libc::pthread_self() })
            .unwrap();

        for mutex in thread_info.held_mutexes {
            let mutex_info = state.local.mutexes.get_mut(&mutex).unwrap();
            // Mark the thread as poisoned
            mutex_info.status = MutexStatus::Poisoned;
            // Remove this thread from the queue
            assert!(mutex_info.queued_threads.pop_front().is_some());
            // If there are any other threads listening, mark the next one as ready to receive
            // the (poisoned) mutex
            if let Some(other_thread) = mutex_info.queued_threads.front().cloned() {
                state.mark_thread_ready(other_thread);
            }
        }

        // Delegate execution to...
        if let Some(thread_id) = state.local.pthreads.values().next().map(|t| t.id) {
            // ...another running thread in this process
            drop(state);
            Scheduler::wake_thread(ctx, &thread_id);
            // SAFETY: `state` is never held from this point onward
            match method {
                TerminationMethod::Cancellation => unsafe {
                    libc::pthread_cancel(libc::pthread_self());
                    libc::sleep(1); // Acts as a backup cancellation point in case `pthread_cancel` didn't work
                    unreachable!("`pthread_cancel` failed to kill current thread");
                },
                TerminationMethod::ThreadExit(retval) => unsafe { libc::pthread_exit(retval) },
                TerminationMethod::Signal(raised_info) => unsafe {
                    libc::pthread_kill(libc::pthread_self(), raised_info.signum());
                    libc::sleep(1); // Acts as a backup cancellation point in case `pthread_kill` didn't work
                    unreachable!("`pthread_kill` failed to kill current thread");
                },
                TerminationMethod::ProcessExit(_) | TerminationMethod::ProcessImmediateExit(_) => {
                    unreachable!("terminate_thread() incorrectly used for process exit")
                }
            }
        } else {
            // ...another process, as this process is going out of scope.
            drop(state);
            // SAFETY: `state` isn't held from this point on
            Scheduler::terminate_process(ctx, method);
        }

        // TODO: What about when the main thread exits normally? Needs atexit() handler installed when process first created...
    }

    /// Removes any global state associated with the given process.
    fn terminate_process(ctx: &mut FizzleSingleton, method: TerminationMethod) -> ! {
        // TODO: remove all active file descriptors, handles from local state so they're freed from global

        let on_exit_val = match method {
            TerminationMethod::ThreadExit(_) => Some(0),
            TerminationMethod::ProcessExit(val) => Some(val),
            _ => None,
        };

        // handle registered `atexit()`/`on_exit()` functions
        if let Some(on_exit_val) = on_exit_val {
            let mut state = ctx.acquire();

            let mut atexit_handlers = Vec::new();
            let mut on_exit_handlers = Vec::new();
            mem::swap(&mut atexit_handlers, &mut state.local.atexit_handlers);
            mem::swap(&mut on_exit_handlers, &mut state.local.on_exit_handlers);
            drop(state);

            for handler in atexit_handlers {
                Scheduler::run_outside_hook(ctx, || unsafe {
                    handler();
                });
            }

            for (handler, arg) in on_exit_handlers {
                Scheduler::run_outside_hook(ctx, || unsafe {
                    handler(on_exit_val, arg);
                });
            }
        }

        // Clean up process state
        let state = ctx.acquire();
        let process_id = state.local.process_id;
        assert!(
            !process_id.is_main_process(),
            "main process forcibly terminated"
        );

        let pid = state.global.processes.get(&process_id).unwrap().pid.as_id();

        let sigchild = match &method {
            TerminationMethod::Cancellation | TerminationMethod::ThreadExit(_) => SigChildInfo {
                code: SigChildCode::Exited,
                pid,
                uid: unsafe { libc::getuid() },
                status: 0,
            },
            TerminationMethod::ProcessExit(val) | TerminationMethod::ProcessImmediateExit(val) => {
                SigChildInfo {
                    code: SigChildCode::Exited,
                    pid,
                    uid: unsafe { libc::getuid() },
                    status: *val,
                }
            }
            TerminationMethod::Signal(raised_info) => SigChildInfo {
                code: SigChildCode::Killed,
                pid: raised_info.pid().unwrap_or(pid),
                uid: raised_info.uid().unwrap_or(unsafe { libc::getuid() }),
                status: raised_info.signum(),
            },
        };

        let parent_id = state.global.processes.get(&process_id).unwrap().ppid;

        drop(state);

        // Send SIGCHLD to parent process
        // NOTE: it is VERY IMPORTANT that this go towards the end of process termination.
        if parent_id != WorkerId::init_process() {
            Scheduler::handle_event(
                ctx,
                SignalSendEvent::new(SignalTarget::Pid(parent_id), libc::SIGCHLD, None),
            )
            .unwrap();
        }

        let mut state = ctx.acquire();

        // Mark this process as a zombie
        // NOTE: it is VERY IMPORTANT that this block comes AFTER the SIGCHLD signal is sent.
        // The reason for this is that `Scheduler::handle_event()` will pause the execution
        // of this process to run signal handlers in the target process/thread of the signal.
        // If userspace code access resources from this process, bad things could happen (?).
        state
            .global
            .exited_processes
            .insert(process_id, sigchild.clone())
            .unwrap();

        // If a parent is awaiting this process's death, notify it
        let awaiting = state
            .global
            .awaiting_process_death
            .remove(&process_id)
            .unwrap();
        for awaiting_worker in awaiting.iter() {
            state.global.mark_worker_ready(*awaiting_worker);
        }

        // DO NOT remove pid or ppid info--they will happen when the process is reaped
        /*
        // Remove the PID of the current process
        state.global.pids.remove(&pid);

        // Remove process info
        state.global.processes.downref(&process_id);
        */

        // TODO: if a parent dies before a child does, the child will never be reaped.
        // Fix this...

        // Clean up this process's semaphore
        state.global.process_locks[usize::from(process_id)] = None;

        // TODO: other global cleanup (such as of socket state from dropped fds) here

        // Delegate execution to the primary process (it's guaranteed not to exit)
        let delegate_process_id = ProcessId::main_process();

        drop(state);
        Scheduler::wake_process(ctx, delegate_process_id);

        match method {
            TerminationMethod::Cancellation => unsafe {
                // This is the last thread in the group
                libc::pthread_cancel(libc::pthread_self());
                libc::sleep(1); // Acts as a backup cancellation point in case `pthread_cancel` didn't work
                panic!("`pthread_cancel()` failed to kill current thread");
            },
            TerminationMethod::ThreadExit(retval) => unsafe { libc::pthread_exit(retval) },
            TerminationMethod::ProcessExit(retval) => unsafe { libc::_exit(retval) },
            TerminationMethod::ProcessImmediateExit(retval) => unsafe { libc::_exit(retval) },
            TerminationMethod::Signal(signal) => unsafe {
                // TODO: if SIGKILL (or any other signal that doesn't run `atexit()`), make sure our atexit handlers still work
                libc::kill(libc::getpid(), signal.signum());
                libc::sleep(1); // Acts as a backup cancellation point in case `pthread_kill` didn't work
                panic!("`pthread_kill()` failed to kill current thread");
            },
        }
    }

    /// Runs the given routine outside of the context of the current method hook.
    ///
    /// Any system library calls performed by code within this closure will be hooked and handled
    /// as if it were being run by the program.
    pub fn run_outside_hook<F, R>(_ctx: &mut FizzleSingleton, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // This should only be called within Fizzle
        debug_assert!(state::has_entered_handler());
        state::set_entered_handler(false);

        // Run the supplied closure outside handler bbounds
        let ret = f();

        // Restore handler bounds
        state::set_entered_handler(true);
        ret
    }
}

pub enum DelegationAction {
    PauseCurrentWorker(DelegationSource),
    RunNextWorker,
    RunThread(ThreadId),
    RunProcess(ProcessId),
}

pub enum DelegationState {
    PauseCurrentWorker(DelegationSource),
    RunNextWorker,
    NoMoreWorkers,
    RunCurrentWorker,
    RunThread(ThreadId),
    RunProcess(ProcessId),
    RunPlugins,
    TerminateThread(TerminationMethod),
    SignalToThread(WorkerId, RaisedSignalInfo),
    HandleSignal(RaisedSignalInfo),
}

impl<'a> From<DelegationAction> for DelegationState {
    #[inline]
    fn from(value: DelegationAction) -> Self {
        match value {
            DelegationAction::PauseCurrentWorker(src) => Self::PauseCurrentWorker(src),
            DelegationAction::RunNextWorker => Self::RunNextWorker,
            DelegationAction::RunThread(t) => Self::RunThread(t),
            DelegationAction::RunProcess(p) => Self::RunProcess(p),
        }
    }
}

#[derive(Clone, Copy)]
pub enum DelegationSource {
    Thread,
    Process,
}

#[derive(Clone, Debug)]
pub enum TerminationMethod {
    Cancellation,
    ProcessExit(i32),
    ProcessImmediateExit(i32),
    ThreadExit(*mut libc::c_void),
    Signal(RaisedSignalInfo),
}
