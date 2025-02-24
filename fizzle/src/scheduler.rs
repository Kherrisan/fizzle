use std::cell::{RefCell, RefMut};
use std::collections::BTreeMap;
use std::os::fd::RawFd;
use std::process::{self, Command};
use std::rc::Rc;
use std::thread::ThreadId;
use std::time::Duration;
use std::{cmp, env, mem, ptr, slice, thread};

use embedded_alloc::TlsfHeap;

use crate::backend::{ConnectedBackend, FileBackend, FileFeedback, PendingBackend};
use crate::cell::{PanicOnceCell, SequentialRefCell};
use crate::constants::{
    FIZZLE_ALLOC_ENV, FIZZLE_ALLOC_OFFSET_ENV, FIZZLE_HEAP_SIZE, FIZZLE_MEMORY_ENV,
    FIZZLE_SINGLEPROCESS_ENV,
};
use crate::errno::Errno;
use crate::handlers::file::{CowInfo, FileInfo};
use crate::handlers::id::Worker;
use crate::handlers::mutex::MutexStatus;
use crate::handlers::poller::PollerInfo;
use crate::handlers::process::*;
use crate::handlers::signal::*;
use crate::handlers::socket::SocketState;
use crate::handlers::time::ItimerInfo;
use crate::state;
use crate::state::*;
use crate::GlobalHeap;
use crate::{plugins, GlobalRc};

static FIZZLE_STATE: PanicOnceCell<SequentialRefCell<FizzleState>> = PanicOnceCell::new();

static FIZZLE_ALLOC: PanicOnceCell<&'static InterprocessAllocator> = PanicOnceCell::new();

#[allow(non_snake_case)]
pub const fn CMSG_ALIGN(len: usize) -> usize {
    len + mem::size_of::<usize>() - 1 & !(mem::size_of::<usize>() - 1)
}

pub struct FizzleSingleton {
    /// Empty private field to ensure `FizzleSingleton` isn't constructed outside of
    /// `fizzle_singleton()`.
    _private: (),
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

pub fn fizzle_alloc() -> GlobalHeap {
    &FIZZLE_ALLOC
        .get_or_init(|| {
            let size = mem::size_of::<InterprocessAllocator>();
            let is_singleprocess =
                matches!(env::var(FIZZLE_SINGLEPROCESS_ENV), Ok(s) if s.as_str() == "1");

            let location = if is_singleprocess {
                let loc = unsafe {
                    libc::mmap(
                        ptr::null_mut(),
                        size,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                        -1,
                        0,
                    )
                };

                if loc == libc::MAP_FAILED {
                    panic!(
                        "failed to mmap InterprocessAllocator memory (errno {})",
                        Errno::get_errno()
                    )
                }

                loc.cast::<InterprocessAllocator>()
            } else {
                // Shared memory doesn't play well with the forkserver, so we need to make sure that
                // processes are forked *before* any shared memory is created.

                #[cfg(all(feature = "afl", feature = "pcr"))]
                unsafe {    
                    crate::__afl_sharedmem_fuzzing = 1;
                }

                #[cfg(feature = "afl")]
                unsafe {
                    crate::__afl_manual_init();
                }

                let memfd = match env::var(FIZZLE_ALLOC_ENV) {
                    Ok(var) => {
                        let memfd: RawFd = var.parse().unwrap();
                        memfd
                    }
                    Err(_) => {
                        let filename = format!("/Fizzle_Alloc{}\0", process::id());

                        let fd = unsafe {
                            libc::shm_open(
                                filename.as_ptr().cast::<i8>(),
                                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                                libc::S_IRUSR | libc::S_IWUSR,
                            )
                        };

                        assert!(fd >= 0, "shm_open() failed: {}", Errno::get_errno());

                        unsafe {
                            assert_eq!(
                                libc::shm_unlink(filename.as_ptr().cast::<i8>()),
                                0,
                                "shm_unlink() failed: {}",
                                Errno::get_errno()
                            );
                        }

                        let memfd = unsafe { libc::dup(fd) };
                        assert!(
                            memfd >= 0,
                            "dup() failed during InterprocessAllocator creation: {}",
                            Errno::get_errno()
                        );

                        unsafe {
                            assert_eq!(libc::close(fd), 0);
                        }

                        env::set_var(FIZZLE_ALLOC_ENV, memfd.to_string());

                        let ret = unsafe { libc::ftruncate(memfd, size as i64) };
                        assert_eq!(
                            ret,
                            0,
                            "ftruncate() failed for InterprocessAllocator memory: {}",
                            Errno::get_errno()
                        );

                        memfd
                    }
                };

                let alloc_offset = match env::var(FIZZLE_ALLOC_OFFSET_ENV) {
                    Ok(var) => var.parse::<usize>().unwrap() as *mut libc::c_void,
                    Err(_) => ptr::null_mut(),
                };

                let loc = unsafe {
                    libc::mmap(
                        alloc_offset,
                        size,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        memfd,
                        0,
                    )
                };

                if loc == libc::MAP_FAILED {
                    panic!(
                        "failed to mmap InterprocessAllocator memory: {}",
                        Errno::get_errno()
                    );
                }

                if alloc_offset.is_null() {
                    env::set_var(FIZZLE_ALLOC_OFFSET_ENV, loc.addr().to_string());
                }

                loc.cast::<InterprocessAllocator>()
            };

            unsafe {
                *(&raw mut (*location).heap) = TlsfHeap::empty();
                (*location).heap.init(
                    (&raw const (*location).heap_memory) as usize,
                    FIZZLE_HEAP_SIZE,
                );
                &*(location.cast_const())
            }
        })
        .heap
}

impl FizzleSingleton {
    fn new() -> Self {
        FizzleSingleton { _private: () }
    }

    /// Acquires the global shared state for mutable access.
    ///
    /// This access does not involve any atomic or locking operations.
    pub fn acquire(&mut self) -> RefMut<'_, FizzleState> {
        FIZZLE_STATE
            .get_or_init(|| SequentialRefCell::new(FizzleState::new()))
            .borrow_mut()
    }
}

#[derive(Clone, Debug)]
pub enum TerminationMethod {
    Cancellation,
    ProcessExit(i32),
    ProcessImmediateExit(i32),
    ThreadExit(*mut libc::c_void),
    Signal(RaisedSignalInfo),
}

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
    Success(S),
    Error(E),
    RunTask(
        Box<dyn FnOnce(&mut FizzleSingleton) -> TaskResult + Send + 'static, &'static TlsfHeap>,
        YieldUntil,
    ),
    Yield(YieldUntil),
}

pub enum YieldUntil {
    /// Immediately run the current worker
    Immediate,
    /// Schedule the current worker in the standard timestamp-based priority queue.
    Reschedule(Duration),
    /// Do not schedule the current worker for continued execution.
    None,
}

pub enum TaskResult {
    Continue,
    Suspend,
    Return,
}

pub struct Scheduler;

impl Scheduler {
    pub fn handle_event<T: Event>(
        ctx: &mut FizzleSingleton,
        mut event: T,
    ) -> Result<T::Success, T::Error> {
        // Run startup commands if needed
        let mut state = ctx.acquire();
        if let Some(main_state) = state.local.main_state.as_mut() {
            if !main_state.onstartup_commands.is_empty() {
                let mut startup_commands = Vec::new();
                mem::swap(&mut startup_commands, &mut main_state.onstartup_commands);
                drop(state);

                while let Some(onstartup) = startup_commands.pop() {
                    let mut state = ctx.acquire();
                    let current_worker = state.current_worker();
                    state
                        .global
                        .ready_delayed
                        .push_back(ReadyInfo::Worker(current_worker));

                    log::info!(
                        "`Scheduler::run_subprocess()` called for startup command {:?}",
                        onstartup
                    );

                    // Schedule the subprocess to be run
                    state.global.tasks.push_front(Box::new_in(
                        move |ctx| {
                            Scheduler::run_subprocess(ctx, onstartup);
                            TaskResult::Continue
                        },
                        fizzle_alloc(),
                    ));
                    drop(state);

                    Scheduler::yield_worker(ctx);
                    log::info!("`Scheduler::run_subprocess()` complete.");
                }
            } else {
                drop(state);
            }
        } else {
            drop(state);
        }

        // Increment the global time clock
        Scheduler::increment_time(ctx);

        loop {
            log::trace!("next iteration of event.run() loop");
            let mut state = ctx.acquire();
            let (task_opt, until) = match event.run(&mut state) {
                Outcome::Success(s) => return Ok(s),
                Outcome::Error(e) => return Err(e),
                Outcome::RunTask(task, until) => (Some(task), until),
                Outcome::Yield(until) => (None, until),
            };

            if let Some(task) = task_opt {
                state.global.tasks.push_front(task);
            }

            match until {
                YieldUntil::Immediate => continue, // Don't yield the worker or execute any tasks
                YieldUntil::Reschedule(delay) => {
                    let current_worker = state.current_worker();
                    let current_time = state.global.current_time;
                    log::trace!("rescheduling worker {:?}", current_worker);

                    state.global.ready.push(ScheduledItem {
                        info: ReadyInfo::Worker(current_worker),
                        timestamp: current_time + delay,
                    });
                }
                // unused
                /*
                YieldUntil::DelayedReschedule => {
                    let current_worker = state.current_worker();
                    state
                        .global
                        .ready_delayed
                        .push_back(ReadyInfo::Worker(current_worker));
                }
                */
                YieldUntil::None => (),
            }

            drop(state);

            Scheduler::yield_worker(ctx);
        }
    }

    pub fn yield_worker(ctx: &mut FizzleSingleton) {
        log::trace!("yield_worker()");

        let state = ctx.acquire();

        let current_worker = state.current_worker();
        let worker_sem = state
            .global
            .worker_locks
            .get(&current_worker)
            .unwrap()
            .clone();

        drop(state);

        loop {
            // Check for any available task to run
            let task_opt = ctx.acquire().global.tasks.pop_front();

            let waiting_sem = if let Some(run_task) = task_opt {
                log::trace!("running next task...");
                // Immediately
                let wait_on = run_task(ctx);
                // Invariant: `ctx` must NOT be acquired between here and `sem.wait()`

                match wait_on {
                    TaskResult::Continue => None,
                    TaskResult::Suspend => Some(worker_sem.clone()),
                    TaskResult::Return => return, // This worker is ready to execute
                }
            } else if let Some(should_yield) = Scheduler::handle_next_scheduled(ctx) {
                if should_yield {
                    Some(worker_sem.clone())
                } else {
                    None
                }
            } else if Scheduler::plugins_have_output(ctx) {
                None
            } else if let Some(ready) = Scheduler::delayed_worker(ctx) {
                let mut state = ctx.acquire();
                let timestamp = state.global.current_time;
                state.global.ready.push(ScheduledItem {
                    info: ready,
                    timestamp,
                });
                None
            } else if let Some(command) = Scheduler::next_onready_command(ctx) {
                Scheduler::run_subprocess(ctx, command);
                Some(worker_sem.clone())
            } else if Scheduler::remove_perround_endpoints(ctx) {
                None
            } else {
                log::debug!("All tasks completed for the given fuzzing round.");
                Scheduler::round_complete(ctx);
                // New fuzzing round => immediately move on to next task/worker
                None
            };

            if let Some(sem) = waiting_sem {
                // SAFETY: sem.wait() must **ONLY** be called here for thread/process locks
                log::debug!("waiting on sem.wait()...");
                sem.wait();
                log::debug!("sem.wait() returned.");
            }
        }
    }

    /// Returns the worker associated with the poller if input is available for that worker.
    fn poller_ready_worker(poller: GlobalRc<PollerInfo>) -> Option<Worker> {
        let worker = poller.borrow().worker;
        log::trace!(
            "Checking if polling worker {:?} is ready for execution...",
            &worker
        );

        for polled in poller.borrow().raised_events.iter() {
            if polled.borrow().event_raised {
                log::trace!("raised poll event found for worker {:?}", worker);
                return Some(worker);
            }
        }

        log::trace!(
            "no polling events for {:?} were ready--clearing events",
            worker
        );
        poller.borrow_mut().raised_events.clear();

        None
    }

    fn handle_next_scheduled(ctx: &mut FizzleSingleton) -> Option<bool> {
        log::trace!("handle_next_scheduled()");
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();

        while let Some(ScheduledItem { info, timestamp }) = state.global.ready.pop() {
            log::trace!("next ReadyInfo popped off queue");

            if timestamp > state.global.current_time + Duration::from_secs(2) {
                log::info!(
                    "next available worker would suspend execution by {} seconds--moving on",
                    (timestamp - state.global.current_time).as_secs()
                );
                state.global.ready.push(ScheduledItem { info, timestamp });
                return None;
            }

            state.global.current_time = cmp::max(state.global.current_time, timestamp);

            let worker_opt = match info {
                ReadyInfo::Worker(worker) => Some(worker),
                ReadyInfo::Poller(poller) => Scheduler::poller_ready_worker(poller),
                ReadyInfo::Timer(pid, timer_type) => {
                    state.global.tasks.push_front(Box::new_in(
                        move |ctx| Scheduler::handle_expired_timer(ctx, pid, timer_type),
                        fizzle_alloc(),
                    ));

                    return Some(false);
                }
            };

            if let Some(worker) = worker_opt {
                state
                    .global
                    .tasks
                    .push_front(Box::new_in(|_| TaskResult::Return, fizzle_alloc()));
                if worker == current_worker {
                    return Some(false);
                } else {
                    let sem = state.global.worker_locks.get(&worker).unwrap().clone();
                    drop(state);
                    log::trace!("[1] post() to {:?}", worker);
                    sem.post();
                    return Some(true);
                }
            }
        }

        None
    }

    fn plugins_have_output(ctx: &mut FizzleSingleton) -> bool {
        let mut state = ctx.acquire();
        plugins::run_plugins(&mut state)
    }

    fn delayed_worker(ctx: &mut FizzleSingleton) -> Option<ReadyInfo> {
        let mut state = ctx.acquire();
        state.global.ready_delayed.pop_front()
    }

    fn next_onready_command(ctx: &mut FizzleSingleton) -> Option<Command> {
        let mut state = ctx.acquire();

        state
            .local
            .main_state
            .as_mut()
            .unwrap()
            .onready_commands
            .pop()
    }

    fn increment_time(ctx: &mut FizzleSingleton) {
        let mut state = ctx.acquire();

        let idx = state.global.time_fuzz_idx;
        let increment = if state.global.fuzz_input.is_empty() {
            20
        } else {
            // Randomly between 0 and 3100 microsecond offset
            let offset = ((state.global.fuzz_input[idx] ^ 0x7f) / 8) as u64;
            state.global.time_fuzz_idx = (idx + 1) % state.global.fuzz_input.len();
            offset
        };

        state.global.current_time += Duration::from_micros(increment * 100);
    }

    pub fn handle_process_signal(
        ctx: &mut FizzleSingleton,
        raised: RaisedSignalInfo,
        dst: Pid,
    ) -> TaskResult {
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();

        // Check to see if the signal should be ignored
        let disposition = state
            .global
            .pids
            .get(&dst)
            .unwrap()
            .borrow()
            .signal_handlers[raised.signum() as usize - 1]
            .clone();

        if dst != current_worker.pid {
            // Re-schedule the current task to run in the destination process
            state.global.tasks.push_front(Box::new_in(
                move |ctx| Scheduler::handle_process_signal(ctx, raised, dst),
                fizzle_alloc(),
            ));

            // Awaken destination process
            let dst_sem = state
                .global
                .pids
                .get(&dst)
                .unwrap()
                .borrow()
                .main_worker_lock
                .clone();

            drop(state);
            log::trace!("[2] post() to {:?}", dst);
            dst_sem.post();

            return TaskResult::Suspend;
        }

        if let SigDisposition::Ignore = disposition {
            log::info!(
                "Process-directed signal {} received and ignored",
                raised.signum()
            );
            return TaskResult::Continue;
        }

        // Now select the appropriate thread to handle the signal
        let Some(tid) = Scheduler::unblocked_thread(&mut state, raised.signum()) else {
            // TODO: make sure this gets checked whenever a thread unblocks itself
            log::debug!("Process-directed signal {} received but all threads were blocked--signal set to pending", raised.signum());
            if state.local.pending_signals[raised.signum() as usize - 1]
                .replace(raised)
                .is_some()
            {
                // Signal was already raised--any `sigsuspend()`ed threads should already be notified
                return TaskResult::Continue;
            }

            // Notify any thread waiting with `sigsuspend()`
            let mut ready_worker = None;
            for (tid, siginfo) in state.local.signals.iter_mut() {
                if siginfo.sigsuspend
                    && siginfo
                        .sigwait_set
                        .intersects(SignalSet::from_signum(raised.signum()))
                {
                    siginfo.sigsuspend = false;
                    ready_worker = Some(Worker {
                        pid: current_worker.pid,
                        thread_id: *tid,
                    });
                }
            }

            if let Some(worker) = ready_worker {
                state.mark_worker_ready(worker);
            }

            return TaskResult::Continue;
        };

        if tid != current_worker.thread_id {
            // Re-schedule the task
            state.global.tasks.push_front(Box::new_in(
                move |ctx| Scheduler::handle_local_signal(ctx, raised),
                fizzle_alloc(),
            ));

            // Awaken destination thread
            let dst_worker = Worker {
                pid: current_worker.pid,
                thread_id: tid,
            };
            let dst_sem = state.global.worker_locks.get(&dst_worker).unwrap().clone();
            drop(state);
            log::trace!("[3] post() to {:?}", dst_worker);
            dst_sem.post();

            return TaskResult::Suspend;
        }

        drop(state);
        Scheduler::handle_local_signal(ctx, raised)
    }

    pub fn handle_thread_signal(
        ctx: &mut FizzleSingleton,
        raised: RaisedSignalInfo,
        dst: Worker,
    ) -> TaskResult {
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();

        // Check to see if the signal should be ignored
        let disposition = state
            .global
            .pids
            .get(&dst.pid)
            .unwrap()
            .borrow()
            .signal_handlers[raised.signum() as usize - 1]
            .clone();

        if let SigDisposition::Ignore = disposition {
            log::info!(
                "Thread-directed signal {} received and ignored",
                raised.signum()
            );
            return TaskResult::Continue;
        }

        // Move to the appropriate thread
        if dst != current_worker {
            // Re-schedule current task
            state.global.tasks.push_front(Box::new_in(
                move |ctx| Scheduler::handle_local_signal(ctx, raised),
                fizzle_alloc(),
            ));

            // Awaken destination worker
            let dst_sem = state.global.worker_locks.get(&dst).unwrap().clone();
            drop(state);
            log::trace!("[4] post() to {:?}", dst);
            dst_sem.post();

            return TaskResult::Suspend;
        }

        // Check to see if the signal has been blocked
        let siginfo = state
            .local
            .signals
            .get_mut(&current_worker.thread_id)
            .unwrap();
        if siginfo
            .masked
            .intersects(SignalSet::from_signum(raised.signum()))
        {
            siginfo.pending[raised.signum() as usize - 1] = Some(raised);
            log::debug!(
                "Thread-directed signal {} received but blocked--set to pending",
                raised.signum()
            );

            // If the thread is waiting with `sigsuspend()`, awaken it
            if siginfo.sigsuspend
                && siginfo
                    .sigwait_set
                    .intersects(SignalSet::from_signum(raised.signum()))
            {
                siginfo.sigsuspend = false;
                state.mark_worker_ready(current_worker);
            }

            return TaskResult::Continue;
        }

        drop(state);
        Scheduler::handle_local_signal(ctx, raised)
    }

    fn handle_local_signal(ctx: &mut FizzleSingleton, raised: RaisedSignalInfo) -> TaskResult {
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();

        let proc_siginfo = state.global.pids.get_mut(&current_worker.pid).unwrap();
        let sig_handler =
            proc_siginfo.borrow().signal_handlers[raised.signum() as usize - 1].clone();

        match sig_handler {
            SigDisposition::Action(action) => {
                drop(state);

                let mut siginfo = siginfo_t::from_raised(raised);
                Scheduler::run_outside_hook(ctx, || unsafe {
                    action(raised.signum(), ptr::addr_of_mut!(siginfo), ptr::null_mut());
                });
            }
            SigDisposition::Handler(handler) => {
                drop(state);
                Scheduler::run_outside_hook(ctx, || unsafe {
                    handler(raised.signum());
                });
            }
            SigDisposition::Default => {
                match SignalSet::from_signum(raised.signum()) {
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
                        Scheduler::terminate_process(ctx, TerminationMethod::Signal(raised))
                    }
                }
            }
            SigDisposition::Ignore => unreachable!(), // should have been handled earlier
        }

        TaskResult::Continue
    }

    // TODO: shouldn't this go in `LocalState`??
    fn unblocked_thread(state: &FizzleState, signum: libc::c_int) -> Option<ThreadId> {
        for (thread, siginfo) in state.local.signals.iter() {
            if !siginfo.masked.contains(SignalSet::from_signum(signum)) {
                return Some(*thread);
            }
        }

        None
    }

    pub fn handle_expired_timer(
        ctx: &mut FizzleSingleton,
        pid: Pid,
        timer_type: TimerType,
    ) -> TaskResult {
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();

        state.global.tasks.push_front(Box::new_in(
            move |ctx| Scheduler::handle_expired_timer(ctx, pid, timer_type),
            fizzle_alloc(),
        ));

        // Need to ensure this is executing within the destination process
        if pid != current_worker.pid {
            // Re-schedule current task
            state.global.tasks.push_front(Box::new_in(
                move |ctx| Scheduler::handle_expired_timer(ctx, pid, timer_type),
                fizzle_alloc(),
            ));

            // Awaken destination process
            let sem = state
                .global
                .pids
                .get(&pid)
                .unwrap()
                .borrow()
                .main_worker_lock
                .clone();
            drop(state);
            log::trace!("[5] post() to {:?}", pid);
            sem.post();

            return TaskResult::Suspend;
        }

        let itimer_info = match timer_type {
            TimerType::Prof => state.local.itimer_prof.clone(),
            TimerType::Real => state.local.itimer_real.clone(),
            TimerType::Virtual => state.local.itimer_virtual.clone(),
        };

        // Repeat timer if applicable
        if let Some(ItimerInfo { interval }) = itimer_info {
            let timestamp = state.global.current_time.saturating_add(interval);
            state.global.ready.push(ScheduledItem {
                info: ReadyInfo::Timer(pid, timer_type),
                timestamp,
            })
        }

        // Now handle the timer's signal behavior
        let raised = RaisedSignalInfo::Timer(SigTimerInfo {
            signum: timer_type.signum(),
            overrun: 0, // TODO: implement correctly
            timer_id: timer_type.timer_id(),
        });

        drop(state);
        Scheduler::handle_process_signal(ctx, raised, pid)
    }

    // TODO: clean this up better
    fn round_complete(ctx: &mut FizzleSingleton) {
        let mut state = ctx.acquire();

        Scheduler::prepare_fuzz_input(&mut state);

        // Reset fuzz endpoint state (e.g. endpoints configured with the `fuzz` option)
        let mut polled_ready = Vec::new_in(fizzle_alloc());
        for endpoint_info in state.global.fuzz_endpoints.iter_mut() {
            endpoint_info.read_idx = 0;
            polled_ready.push(endpoint_info.read_polled.clone());
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
            .iter()
            .map(|plugin_info| plugin_info.borrow().module.clone())
            .collect();
        for module in modules {
            module
                .borrow_mut()
                .fuzz_round_start(state.global.fuzz_input.as_slice());
        }

        let plugins = state.global.plugins.clone();

        for plugin_info in plugins {
            plugin_info.borrow_mut().read_buf.clear();
            plugin_info.borrow_mut().write_buf.clear();

            state.lower_polled(&plugin_info.borrow_mut().read_polled);
            state.raise_polled(&plugin_info.borrow_mut().write_polled);
        }

        // Reset per-round fuzzing clients
        let per_round_clients = state.global.per_round_clients.clone();

        log::info!(
            "{} per-round clients to be initialized...",
            per_round_clients.len()
        );

        // Add new pending per-round fuzzing clients
        for client_info in per_round_clients {
            let socket_info = state.global.add_pending_client(
                client_info.source_address,
                client_info.target_address,
                match client_info.backend {
                    PerRoundClientBackend::Fuzz(fuzz_endpoint_id) => {
                        PendingBackend::Fuzz(fuzz_endpoint_id)
                    }
                    PerRoundClientBackend::Plugin(plugin_id) => PendingBackend::Plugin(plugin_id),
                },
            );
            log::debug!(
                "added pending client with local addr {:?}",
                socket_info.borrow().local_addr
            );
            state.global.per_round_endpoints.push(socket_info);
        }

        drop(state);
    }

    fn remove_perround_endpoints(ctx: &mut FizzleSingleton) -> bool {
        let mut state = ctx.acquire();

        let endpoints: Vec<_> = state.global.per_round_endpoints.drain(..).collect();
        if endpoints.is_empty() {
            return false;
        }

        for sock_info in endpoints {
            match &mut sock_info.borrow_mut().state {
                SocketState::PendingConnection(pending) => {
                    let addr = pending.rem_addr.clone();
                    // Pending sockets are exclusively the result of per-round clients, so we just clear() wholesale here.
                    state
                        .global
                        .socket_locations
                        .get_mut(&addr)
                        .unwrap()
                        .pending
                        .clear();
                }
                SocketState::Connected(connected) => {
                    log::debug!("removing connected fuzz/plugin client socket");
                    if !connected.peer_closed {
                        connected.peer_closed = true;

                        // Now raise all applicable poll events so the reader discovers the peer is closed
                        match connected.backend.clone() {
                            ConnectedBackend::Plugin(plugin_info) => {
                                let read_polled = plugin_info.borrow().read_polled.clone();
                                state.raise_polled(&read_polled);
                            }
                            ConnectedBackend::Fuzz(fuzz_endpoint) => {
                                let read_polled = fuzz_endpoint.borrow().read_polled.clone();
                                state.raise_polled(&read_polled);
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                _ => unreachable!(),
            }
        }

        true
    }

    fn prepare_fuzz_input(state: &mut FizzleState) {
        #[cfg(feature = "afl")]
        if !state.global.afl_shmem_initialized {
            state.global.afl_shmem_initialized = true;

            #[cfg(feature = "pcr")]
            unsafe {
                crate::__afl_sharedmem_fuzzing = 1;
            }

            unsafe {
                crate::__afl_manual_init();
            }
        }

        #[cfg(not(feature = "pcr"))]
        if !state.global.fuzz_input.is_empty() {
            // 
            unsafe {
                libc::_exit(0);
            }
        }

        // Deallocate the last buffer
        state.global.fuzz_input.clear();

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

            if crate::__afl_fuzz_ptr.is_null() {
                panic!("__afl_fuzz_ptr was null--is shared-memory fuzzing enabled?")
                /*
                let read_amount =
                    libc::read(0, fuzz_buffer.as_mut_ptr().cast::<libc::c_void>(), 1048576);
                *crate::__afl_fuzz_len = (read_amount & u32::MAX as isize) as u32;
                if read_amount < 0 {
                    panic!("could not read input from stdin")
                }
                */
            } else {
                let afl_buf =
                    slice::from_raw_parts(crate::__afl_fuzz_ptr, *crate::__afl_fuzz_len as usize);
                state.global.fuzz_input.extend_from_slice(afl_buf);

                state
                    .global
                    .fuzz_input
                    .set_len(*crate::__afl_fuzz_len as usize);
            };
        }

        #[cfg(not(feature = "pcr"))]
        loop {
            let prev_len = state.global.fuzz_input.len();
            let spare = state.global.fuzz_input.spare_capacity_mut().len();
            if spare < 16384 {
                state.global.fuzz_input.reserve(16384 - spare);
            }

            let spare_mut = state.global.fuzz_input.spare_capacity_mut();
            assert!(!spare_mut.is_empty());

            match unsafe { libc::read(0, spare_mut.as_mut_ptr().cast::<libc::c_void>(), spare_mut.len()) } {
                ..=-1 => panic!("read() failed for fuzzing: errno {}", Errno::get_errno()),
                0 => break,
                read_amount => unsafe {
                    state.global.fuzz_input.set_len(prev_len + read_amount as usize);
                }
            }
        }

        if state.global.fuzz_input.is_empty() {
            log::error!("failed to read any fuzzing input in--exiting");
            unsafe { libc::_exit(0) }
        }

        state.global.time_fuzz_idx = 0;
    }

    // TODO: this must be consistent with `fork()`/`execve()` and similar.

    /// Executes the provided command as a child process within the Fizzle harness.
    fn run_subprocess(ctx: &mut FizzleSingleton, mut cmd: Command) {
        // TODO: need to upref all reference-counted (non-CLOEXEC) global variables here

        // TODO: need to prepare signal handlers to be passed
        let mut state = ctx.acquire();

        // Initialize AFL (forkservers and multi-process applications don't play well)
        #[cfg(feature = "afl")]
        if !state.global.afl_shmem_initialized {
            state.global.afl_shmem_initialized = true;

            #[cfg(feature = "pcr")]
            unsafe {
                crate::__afl_sharedmem_fuzzing = 1;
            }

            log::debug!("calling __afl_manual_init()");
            unsafe {
                crate::__afl_manual_init();
            }
            log::debug!("__afl_manual_init finished");
        }

        let parent_info = state.local.process_info.clone();
        let ppid = Pid::INIT;
        let signal_handlers = parent_info.borrow().signal_handlers.clone();

        // Assign a pid to this process--use parent ProcessId TEMPORARILY until child id assigned
        let new_pid = state.global.next_pid();
        let pgid = Pgid::from_pid(new_pid);

        state.global.inherited_state = Some(InheritedState {
            fds: BTreeMap::new_in(fizzle_alloc()),
            pid: new_pid,
            ppid,
            pgid,
            signal_handlers,
            sigmask: SignalSet::empty(), // TODO: is this correct?
        });

        drop(state);

        cmd.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
        cmd.env(FIZZLE_MEMORY_ENV, std::env::var(FIZZLE_MEMORY_ENV).unwrap());
        cmd.env(FIZZLE_ALLOC_ENV, std::env::var(FIZZLE_ALLOC_ENV).unwrap());
        cmd.spawn().unwrap();
    }

    /// Terminates the current thread, cleaning up its resources along the way.
    pub fn terminate_thread(ctx: &mut FizzleSingleton, method: TerminationMethod) -> ! {
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
        let total = cleanup_routines.len();
        for (i, routine) in cleanup_routines.into_iter().enumerate() {
            log::debug!(
                "pthread_key or cleanup routine {} of {} running...",
                i + 1,
                total
            );
            Self::run_outside_hook(ctx, || {
                routine.call();
            });
            log::debug!("routine {} of {} complete.", i + 1, total);
        }

        let mut state = ctx.acquire();
        let current_worker = state.current_worker();

        // Clean up this thread's semaphore
        state.global.worker_locks.remove(&current_worker);

        // Mark this thread as dead for future threads that may wait on it.
        state.local.terminated_threads.insert(thread_id);

        // Notify any threads awaiting this thread's death
        if let Some(awaiting_threads) = state.local.awaiting_thread_death.remove(&thread_id) {
            for thread_id in awaiting_threads {
                state.mark_thread_ready(thread_id);
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
            let worker = Worker {
                pid: current_worker.pid,
                thread_id,
            };
            let sem = state.global.worker_locks.get(&worker).unwrap().clone();
            drop(state);

            log::trace!("[6] post() to {:?}", worker);

            // Wake thread
            sem.post();
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
    pub fn terminate_process(ctx: &mut FizzleSingleton, method: TerminationMethod) -> ! {
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
        let pid = state.local.process_info.borrow().pid;
        let ppid = state.local.process_info.borrow().ppid;

        if pid == Pid::PRIMARY {
            log::error!("main process forcibly terminated");
            unsafe {
                libc::_exit(1);
            }
        }

        let sigchild = match &method {
            TerminationMethod::Cancellation | TerminationMethod::ThreadExit(_) => SigChildInfo {
                code: SigChildCode::Exited,
                pid: pid.as_raw(),
                uid: unsafe { libc::getuid() },
                status: 0,
            },
            TerminationMethod::ProcessExit(val) | TerminationMethod::ProcessImmediateExit(val) => {
                SigChildInfo {
                    code: SigChildCode::Exited,
                    pid: pid.as_raw(),
                    uid: unsafe { libc::getuid() },
                    status: *val,
                }
            }
            TerminationMethod::Signal(raised_info) => SigChildInfo {
                code: SigChildCode::Killed,
                pid: raised_info.pid().unwrap_or(pid.as_raw()),
                uid: raised_info.uid().unwrap_or(unsafe { libc::getuid() }),
                status: raised_info.signum(),
            },
        };

        drop(state);

        // Send SIGCHLD to parent process
        // NOTE: it is VERY IMPORTANT that this go towards the end of process termination.
        if ppid != Pid::INIT {
            log::info!("sending SIGCHLD to parent process {}", ppid.as_raw());
            // TODO: if ppid == Pid::INIT, things should be reaped
            Scheduler::handle_event(
                ctx,
                SignalSendEvent::new(SignalTarget::Pid(ppid), libc::SIGCHLD, None),
            )
            .unwrap();
        }

        let mut state = ctx.acquire();

        // Mark this process as a zombie
        // NOTE: it is VERY IMPORTANT that this block comes AFTER the SIGCHLD signal is sent.
        // The reason for this is that `Scheduler::handle_event()` will pause the execution
        // of this process to run signal handlers in the target process/thread of the signal.
        // If userspace code access resources from this process, bad things could happen (?).
        state.global.dead_pids.insert(pid, sigchild.clone());

        // If a parent is awaiting this process's death, notify it
        let awaiting = state.local.process_info.borrow_mut().awaiting_death.take();
        if let Some(awaiting_worker) = awaiting {
            state.mark_worker_ready(awaiting_worker);
        }

        state.global.pids.remove(&pid);
        // TODO: mark process as able to be reaped

        // TODO: if a parent dies before a child does, the child will never be reaped.
        // Fix this...

        // TODO: other global cleanup (such as of socket state from dropped fds) here

        // Delegate execution to the primary process (it's guaranteed not to exit)
        let delegate_sem = state
            .global
            .pids
            .get(&Pid::PRIMARY)
            .unwrap()
            .borrow()
            .main_worker_lock
            .clone();

        log::info!("Exiting process and delegating to main semaphore");

        drop(state);

        log::trace!("[7] post() to primary Pid");
        delegate_sem.post();

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

    pub fn create_cow(state: &mut FizzleState, source: &CreateCowSource) {
        let origin_worker = state.current_worker();
        let origin_pid = state.local.process_info.borrow().pid;

        let move_to_primary = if origin_pid != Pid::PRIMARY {
            Some(Box::new_in(
                move |ctx: &mut FizzleSingleton| {
                    let state = ctx.acquire();
                    let sem = state
                        .global
                        .pids
                        .get(&Pid::PRIMARY)
                        .unwrap()
                        .borrow()
                        .main_worker_lock
                        .clone();
                    drop(state);

                    log::trace!("[8] post() to primary Pid");
                    sem.post();
                    TaskResult::Suspend
                },
                fizzle_alloc(),
            ))
        } else {
            None
        };

        let cow_source = source.clone();
        let create_cow_in_primary = Box::new_in(
            move |ctx: &mut FizzleSingleton| {
                let mut state = ctx.acquire();

                let CreateCowSource::New(path, mode) = cow_source else {
                    return TaskResult::Continue;
                };

                // Create a CoW
                let cow_id = state.allocate_cow();

                let inode = state.global.next_inode();
                let current_time = state.global.current_time;
                let uid = state.global.uid;
                let gid = state.global.gid;

                if !state.global.file_paths.contains_key(&path) {
                    let file_info = Rc::new_in(
                        RefCell::new(FileInfo {
                            path: path.clone(),
                            cow: Some(cow_id),
                            dev_id: 0xfe01,
                            inode,
                            mode,
                            nlink: 1, // TODO: fix
                            backend: FileBackend::Feedback(FileFeedback {}),
                            uid,
                            gid,
                            atime: current_time,
                            btime: current_time,
                            mtime: current_time,
                            ctime: current_time,
                        }),
                        fizzle_alloc(),
                    );

                    if state
                        .global
                        .file_paths
                        .insert(path.clone(), file_info)
                        .is_err()
                    {
                        panic!("failed to insert to file_paths")
                    }
                } else {
                    let file_info = state.global.file_paths.get(&path).unwrap().clone();
                    file_info.borrow_mut().cow = Some(cow_id);
                }

                let memfd = state.local.pasture.get(&cow_id).unwrap().memfd;
                copy_to_shmem(memfd, &path);

                TaskResult::Continue
            },
            fizzle_alloc(),
        );

        let cow_source = source.clone();
        let send_cow_to_origin = if origin_pid != Pid::PRIMARY {
            Some(Box::new_in(
                move |ctx: &mut FizzleSingleton| {
                    let state = ctx.acquire();

                    let cow_id = match cow_source {
                        CreateCowSource::Existing(cow_id) => cow_id,
                        CreateCowSource::New(path, _mode) => {
                            let file_info = state.global.file_paths.get(&path).unwrap();
                            file_info.borrow().cow.unwrap()
                        }
                    };
                    let memfd = state.local.pasture.get(&cow_id).unwrap().memfd;

                    let cmsghdr = libc::cmsghdr {
                        cmsg_len: mem::size_of::<libc::cmsghdr>() + mem::size_of::<RawFd>(),
                        cmsg_level: libc::SCM_RIGHTS,
                        cmsg_type: libc::SOL_SOCKET,
                    };

                    let mut control =
                        [0u8; mem::size_of::<libc::cmsghdr>() + mem::size_of::<RawFd>()];
                    control[..mem::size_of::<libc::cmsghdr>()]
                        .copy_from_slice(unsafe { slice::from_ref(&cmsghdr).align_to::<u8>().1 });
                    control[mem::size_of::<libc::cmsghdr>()..]
                        .copy_from_slice(&memfd.to_ne_bytes());

                    let msghdr = libc::msghdr {
                        msg_name: ptr::null_mut(),
                        msg_namelen: 0,
                        msg_iov: ptr::null_mut(),
                        msg_iovlen: 0,
                        msg_control: control.as_mut_ptr().cast::<libc::c_void>(),
                        msg_controllen: control.len(),
                        msg_flags: 0,
                    };

                    let len = unsafe {
                        libc::sendmsg(state.global.unix_write_fd, ptr::addr_of!(msghdr), 0)
                    };

                    assert_eq!(len, 0);

                    TaskResult::Continue
                },
                fizzle_alloc(),
            ))
        } else {
            None
        };

        let move_to_origin = if origin_pid != Pid::PRIMARY {
            Some(Box::new_in(
                move |ctx: &mut FizzleSingleton| {
                    let state = ctx.acquire();
                    let sem = state
                        .global
                        .worker_locks
                        .get(&origin_worker)
                        .unwrap()
                        .clone();
                    drop(state);

                    log::trace!("[9] post() to {:?}", origin_worker);
                    sem.post();
                    TaskResult::Suspend
                },
                fizzle_alloc(),
            ))
        } else {
            None
        };

        let cow_source = source.clone();
        let recv_cow_at_origin = if origin_pid != Pid::PRIMARY {
            Some(Box::new_in(
                move |ctx: &mut FizzleSingleton| {
                    let mut state = ctx.acquire();

                    let cow_id = match cow_source {
                        CreateCowSource::Existing(cow_id) => cow_id,
                        CreateCowSource::New(path, _mode) => {
                            let file_info = state.global.file_paths.get(&path).unwrap();
                            file_info.borrow().cow.unwrap()
                        }
                    };

                    let mut msg = [0u8; 1024];

                    let mut msghdr = libc::msghdr {
                        msg_name: ptr::null_mut(),
                        msg_namelen: 0,
                        msg_iov: ptr::null_mut(),
                        msg_iovlen: 0,
                        msg_control: msg.as_mut_ptr().cast::<libc::c_void>(),
                        msg_controllen: 1024,
                        msg_flags: 0,
                    };

                    unsafe {
                        assert_eq!(
                            libc::recvmsg(state.global.unix_read_fd, ptr::addr_of_mut!(msghdr), 0),
                            0
                        );
                    }

                    let msg_len = msghdr.msg_controllen;
                    let mut msg_idx = 0;

                    while msg_len - msg_idx > mem::size_of::<libc::cmsghdr>() {
                        let (s1, m, _s2) = unsafe { msg[msg_idx..].align_to::<libc::cmsghdr>() };
                        assert!(s1.is_empty());
                        let hdr = &m[0];
                        if hdr.cmsg_len > msg_len {
                            break;
                        }

                        if hdr.cmsg_type == libc::SOL_SOCKET && hdr.cmsg_level == libc::SCM_RIGHTS {
                            let msg_data = &msg
                                [msg_idx + mem::size_of::<libc::cmsghdr>()..msg_idx + hdr.cmsg_len];
                            let (s1, fds, s2) = unsafe { msg_data.align_to::<RawFd>() };
                            assert!(s1.is_empty() && s2.is_empty() && fds.len() == 1);

                            state
                                .local
                                .pasture
                                .insert(cow_id, CowInfo { memfd: fds[0] });

                            return TaskResult::Continue;
                        }

                        // Update msg index
                        msg_idx = cmp::max(
                            CMSG_ALIGN(msg_idx + hdr.cmsg_len),
                            CMSG_ALIGN(msg_idx + mem::size_of::<libc::cmsghdr>()),
                        );

                        if msg_idx > msg_len {
                            break;
                        }
                    }

                    unreachable!("CoW msg had no SCM_RIGHTS")
                },
                fizzle_alloc(),
            ))
        } else {
            None
        };

        // Tasks need to be pushed on in reverse order of execution
        if let Some(task) = recv_cow_at_origin {
            state.global.tasks.push_front(task);
        }

        if let Some(task) = move_to_origin {
            state.global.tasks.push_front(task);
        }

        if let Some(task) = send_cow_to_origin {
            state.global.tasks.push_front(task);
        }

        state.global.tasks.push_front(create_cow_in_primary);

        if let Some(task) = move_to_primary {
            state.global.tasks.push_front(task);
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
