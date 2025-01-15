use core::slice;
use std::cell::{RefCell, RefMut};
use std::collections::BTreeMap;
use std::os::fd::RawFd;
use std::process::{self, Command};
use std::thread::ThreadId;
use std::time::Duration;
use std::{cmp, env, mem, ptr, thread};

use embedded_alloc::TlsfHeap;

use crate::backend::{ConnectedBackend, FileBackend, FileFeedback, PendingBackend};
use crate::cell::{PanicOnceCell, SequentialRefCell};
use crate::constants::{FIZZLE_ALLOC_ENV, FIZZLE_HEAP_SIZE, FIZZLE_MEMORY_ENV, FIZZLE_SINGLEPROCESS_ENV};
use crate::errno::Errno;
use crate::handlers::file::{CowInfo, FileInfo};
use crate::handlers::mutex::MutexStatus;
use crate::handlers::process::*;
use crate::handlers::signal::*;
use crate::handlers::socket::{SocketInfo, SocketState};
use crate::semaphore::Semaphore;
use crate::{plugins, GlobalRc};
use crate::state;
use crate::state::*;

static FIZZLE_STATE: PanicOnceCell<SequentialRefCell<FizzleState>> = PanicOnceCell::new();

static FIZZLE_ALLOC: PanicOnceCell<&'static InterprocessAllocator> = PanicOnceCell::new();

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

pub fn fizzle_alloc() -> &'static TlsfHeap {
    &FIZZLE_ALLOC
        .get_or_init(|| {
            let size = mem::size_of::<InterprocessAllocator>();
            let is_singleprocess =
                matches!(env::var(FIZZLE_SINGLEPROCESS_ENV), Ok(s) if s.as_str() == "1");

            let location = if is_singleprocess {
                let loc = unsafe { libc::mmap(
                    ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )};

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
                            libc::shm_open(filename.as_ptr().cast::<i8>(), libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, libc::S_IRUSR | libc::S_IWUSR)
                        };

                        assert!(fd >= 0, "shm_open() failed: {}", Errno::get_errno());

                        unsafe {
                            assert_eq!(libc::shm_unlink(filename.as_ptr().cast::<i8>()), 0, "shm_unlink() failed: {}", Errno::get_errno());
                        }

                        let memfd = unsafe { libc::dup(fd) };
                        assert!(memfd >= 0, "dup() failed during InterprocessAllocator creation: {}", Errno::get_errno());

                        unsafe {
                            assert_eq!(libc::close(fd), 0);
                        }

                        env::set_var(FIZZLE_ALLOC_ENV, memfd.to_string());

                        let ret = unsafe { libc::ftruncate(memfd, size as i64) };
                        assert_eq!(ret, 0, "ftruncate() failed for InterprocessAllocator memory: {}", Errno::get_errno());

                        memfd
                    }
                };

                let loc = unsafe {
                    libc::mmap(
                        ptr::null_mut(),
                        size,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        memfd,
                        0
                    )
                };

                if loc == libc::MAP_FAILED {
                    panic!("failed to mmap InterprocessAllocator memory: {}", Errno::get_errno());
                }

                loc.cast::<InterprocessAllocator>()
            };

            unsafe {
                *(&raw mut (*location).heap) = TlsfHeap::empty();
                (*location).heap.init((&raw const (*location).heap_memory) as usize, FIZZLE_HEAP_SIZE);
                &*(location.cast_const())
            }
        }).heap
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
            .get_or_init(|| { SequentialRefCell::new(FizzleState::new()) })
            .borrow_mut()
    }
}

#[allow(non_snake_case)]
pub const fn CMSG_ALIGN(len: usize) -> usize {
    len + mem::size_of::<usize>() - 1 & !(mem::size_of::<usize>() - 1)
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
    /// The value S should be returned for the hook function.
    Success(S),
    /// The error value and errno specified in E should be returned for the function.
    Error(E),
    /// Yields the current thread and executes the next ready worker.
    Yield(Option<Duration>),
    /// The event should move on to its next action immediately.
    Continue,
    /// Yields the current thread without executing the next ready worker.
    Pause(DelegationSource, Option<std::rc::Rc<Semaphore, &'static TlsfHeap>>),
    /// Terminates the given thread's execution.
    TerminateThread(TerminationMethod),
    /// Terminates the given process's execution.
    TerminateProcess(TerminationMethod),
    /// Executes the given method.
    // TODO: make this generic in the future.
    Execute(unsafe extern "C" fn()),
    /// Send the given signal to the specified worker
    SendSignal(SignalDestination, RaisedSignalInfo),
    /// Create or migrate a copy-on-write (CoW) file.
    CreateCow(CreateCowSource),
}

pub struct Scheduler;

impl Scheduler {
    pub fn handle_event<T: Event>(
        ctx: &mut FizzleSingleton,
        mut event: T,
    ) -> Result<T::Success, T::Error> {

        // Increment the global time clock
        Scheduler::increment_time(ctx);

        loop {
            // First `acquire()` call for state allocates and instantiates shared memory
            let mut state = ctx.acquire();

            // Run startup commands if needed
            if let Some(main_state) = state.local.main_state.as_mut() {
                if !main_state.onstartup_commands.is_empty() {
                    let mut startup_commands = Vec::new();
                    mem::swap(&mut startup_commands, &mut main_state.onstartup_commands);

                    let curr_proc_sem = state.local.process_info.borrow().semaphore.clone();
                    drop(state);

                    while let Some(onstartup) = startup_commands.pop() {
                        log::info!("`Scheduler::run_subprocess()` called for startup command {:?}", onstartup);
                        Scheduler::run_subprocess(ctx, onstartup);
                        Scheduler::yield_worker(ctx, DelegationAction::PauseCurrentWorker(DelegationSource::Process(curr_proc_sem.clone()), None));
                        log::info!("`Scheduler::run_subprocess()` complete.");
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

                    let timestamp = state.global.current_time.saturating_add(duration);
                    let worker = state.current_worker();

                    state.global.ready.push(ReadyItem {
                        info: ReadyInfo::Worker(worker),
                        timestamp,
                    });
                    drop(state);
                    Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);

                }
                Outcome::Pause(src, sem) => {
                    // SAFETY: `state` is never used prior to being dropped, so noalias isn't violated
                    drop(state);
                    Scheduler::yield_worker(ctx, DelegationAction::PauseCurrentWorker(src, sem));
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
                Outcome::CreateCow(create_cow) => {
                    drop(state);
                    Scheduler::create_cow(ctx, create_cow);
                }
            }
        }
    }

    fn increment_time(ctx: &mut FizzleSingleton) {
        let mut state = ctx.acquire();

        let idx = state.global.time_fuzz_idx;
        let increment = if state.global.fuzz_input.is_empty() {
            20
        } else {
            // Randomly between 0 and 31 millisecond offset
            let offset = ((state.global.fuzz_input[idx] ^ 0x7f) / 8) as u64;
            state.global.time_fuzz_idx = (idx + 1) % state.global.fuzz_input.len();
            offset
        };

        state.global.current_time += Duration::from_millis(increment);
    }

    /// Gives up execution of the current thread until it is rescheduled.
    ///
    /// This should be the **only** method that uses per-thread/process semaphores.
    fn yield_worker(ctx: &mut FizzleSingleton, action: DelegationAction) {
        // SAFETY: `state` must not be accessed prior to 'yielded
        let current_thread_id = thread::current().id();
        let mut delegation_state = DelegationState::from(action);

        // TODO: the control flow of this method has become bad. Insanely bad. Tagged loops within
        // tagged loops??? Variable initializion deep within the control flow of said loops? Add in
        // the hearty use of `continue`s, `return`s and enum-based return values, and this thing is
        // a nightmare.
        //
        // Need to refactor sometime soon...
        'yielded: loop {
            let delegation_source: DelegationSource;
            // 1. Perform a delegation action
            let posted_sem = match delegation_state {
                // The current worker is creating a new thread
                DelegationState::PauseCurrentWorker(src, sem) => {
                    delegation_source = src;
                    sem
                },
                // The current worker is done being yielded
                DelegationState::RunCurrentWorker => return,
                // The current worker is delegating execution to whatever is available
                DelegationState::RunNextWorker => {
                    let mut state = ctx.acquire();
                    let curr_proc_sem = state.local.process_info.borrow().semaphore.clone();

                    let worker_res = 'get_worker: loop {
                        let Some(ReadyItem { info, timestamp }) = state.global.ready.pop() else {
                            delegation_state = DelegationState::NoMoreWorkers;
                            continue 'yielded;
                        };

                        // TODO: make this timeout value configurable (currently 10 seconds)
                        if timestamp > state.global.current_time + Duration::from_secs(10) {
                            state.global.ready.push(ReadyItem { info, timestamp });
                            delegation_state = DelegationState::NoMoreWorkers;
                            continue 'yielded;
                        }
                        if timestamp > state.global.current_time {
                            state.global.current_time = timestamp;
                        }

                        match info {
                            ReadyInfo::Worker(worker) => break 'get_worker Ok(worker),
                            ReadyInfo::Poller(poller) => {
                                let worker = poller.borrow().worker;
                                log::trace!(
                                    "Checking if {:?} is ready for execution...",
                                    &worker
                                );
                                
                                for polled in poller.borrow().raised_events.iter() {
                                    if polled.borrow().event_raised {
                                        log::trace!("{:?} is ready for execution", worker);
                                        break 'get_worker Ok(worker)
                                    }
                                }

                                log::trace!(
                                    "{:?} is not ready for execution--clearing events",
                                    worker
                                );
                                poller.borrow_mut().raised_events.clear();
                            }
                            ReadyInfo::Timer(pid, timer_type) => {
                                let waking_sem = state.global.pids.get(&pid).unwrap().borrow().semaphore.clone();

                                state.global.signal = Some((
                                    SignalDestination::Process(pid),
                                    RaisedSignalInfo::Timer(SigTimerInfo {
                                        signum: match timer_type {
                                            TimerType::Real => libc::SIGALRM,
                                            TimerType::Virtual => libc::SIGVTALRM,
                                            TimerType::Prof => libc::SIGPROF,
                                        },
                                        overrun: 0, // TODO: implement correctly
                                        timer_id: match timer_type {
                                            TimerType::Real => libc::ITIMER_REAL,
                                            TimerType::Virtual => libc::ITIMER_VIRTUAL,
                                            TimerType::Prof => libc::ITIMER_PROF,
                                        },
                                    })
                                ));

                                break 'get_worker Err(waking_sem)
                            }
                        };
                    };

                    match worker_res {
                        Ok(dst_worker) => {
                            let Some(dst_proc_info) = state.global.pids.get(&dst_worker.pid).cloned() else {
                                // The given worker was killed--continue on to the next one
                                continue 'yielded;
                            };

                            let worker_pid = dst_proc_info.borrow().pid;

                            log::debug!("Scheduling next worker for execution");

                            // Give the next process the info it needs to run the correct thread
                            state.global.waking_id = Some(dst_worker.thread_id);
                            let local_pid = dst_proc_info.borrow().pid;

                            if worker_pid != local_pid {
                                // Execution needs to move to another process
                                let sem = dst_proc_info.borrow().semaphore.clone();
                                drop(state);
                                delegation_source = DelegationSource::Process(curr_proc_sem);
                                Some(sem)

                            } else if dst_worker.thread_id != current_thread_id {
                                // Execution needs to move to another thread
                                let curr_thread_sem = state.local.thread_locks.get(&thread::current().id()).unwrap().clone();
                                let sem = state.local.thread_locks.get(&dst_worker.thread_id).unwrap().clone();
                                drop(state);
                                delegation_source = DelegationSource::Thread(curr_thread_sem);
                                Some(sem)

                            } else {
                                state.global.waking_id = None;
                                drop(state);

                                delegation_state = DelegationState::RunCurrentWorker;
                                continue 'yielded;
                            }
                        }
                        Err(sem) => {
                            delegation_source = DelegationSource::Process(curr_proc_sem);
                            Some(sem)
                        },
                    }
                }
                DelegationState::RunProcess(process_id) => {
                    // Immediately awaken the specified process (used during cancellation)

                    let state = ctx.acquire();
                    let sem = state.global.pids.get(&process_id).unwrap().borrow().semaphore.clone();
                    let curr_proc_sem = state.local.process_info.borrow().semaphore.clone();
                    delegation_source = DelegationSource::Process(curr_proc_sem);
                    drop(state);
                    Some(sem)
                }
                DelegationState::RunThread(thread_id) => {
                    // Immediately awaken the specified thread (used during cancellation)
                    let state = ctx.acquire();
                    let sem = state.local.thread_locks.get(&thread_id).unwrap().clone();
                    let curr_thread_sem = state.local.thread_locks.get(&thread::current().id()).unwrap().clone();
                    delegation_source = DelegationSource::Thread(curr_thread_sem);
                    drop(state);
                    Some(sem)
                }
                DelegationState::NoMoreWorkers => {
                    log::debug!("No workers were ready for execution");

                    let state = ctx.acquire();
                    let local_proc_info = state.local.process_info.clone();
                    let pid = local_proc_info.borrow().pid;
                    let main_sem = state.global.pids.get(&Pid::PRIMARY).unwrap().borrow().semaphore.clone();
                    let curr_proc_sem = state.local.process_info.borrow().semaphore.clone();

                    drop(state);

                    // No more workers means it's time for plugins to execute
                    if pid != Pid::PRIMARY {
                        // Execution needs to be moved to the main process
                        delegation_source = DelegationSource::Process(curr_proc_sem);
                        Some(main_sem)

                    } else {
                        // Execution is already in the main process
                        delegation_state = DelegationState::RunPlugins;
                        continue 'yielded;
                    }
                }
                DelegationState::RunPlugins => {
                    let mut state = ctx.acquire();
                    
                    assert_eq!(state.local.process_info.borrow().pid, Pid::PRIMARY);

                    if plugins::run_plugins(&mut state) {
                        // There are outstanding inputs from plugins to be processed
                        delegation_state = DelegationState::RunNextWorker;
                        continue 'yielded;

                    } else if let Some(ready) = state.global.ready_delayed.pop_front() {
                        let timestamp = state.global.current_time;
                        // There are outstanding delayed workers
                        state.global.ready.push(ReadyItem {
                            info: ready,
                            timestamp,
                        });
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
                        let curr_proc_sem = state.local.process_info.borrow().semaphore.clone();

                        // Safety: this drop MUST occur before `run_subprocess()`
                        drop(state);
                        Scheduler::run_subprocess(ctx, onready);
                        delegation_source = DelegationSource::Process(curr_proc_sem);
                        None

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

                        delegation_state = DelegationState::RunNextWorker;
                        continue 'yielded;
                    }
                }
                DelegationState::TerminateThread(method) => {
                    Scheduler::terminate_thread(ctx, method);
                }
                DelegationState::SignalToThread(pid, thread_id, signal) => {
                    let mut state = ctx.acquire();

                    if thread_id != thread::current().id() {
                        let curr_thread_sem = state.local.thread_locks.get(&thread::current().id()).unwrap().clone();
                        state.global.signal = Some((SignalDestination::Thread(pid, thread_id), signal));
                        delegation_source = DelegationSource::Thread(curr_thread_sem);
                        let sem = state.local.thread_locks.get(&thread_id).unwrap().clone();
                        drop(state);
                        Some(sem)

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
            };

            // SAFETY: no mutable references to `state` being held here

            // 2. Suspend execution

            let waiting_sem = match delegation_source {
                DelegationSource::Thread(sem) => sem,
                DelegationSource::Process(sem) => sem,
            };

            // Awaken the next thread to be run
            if let Some(posting_sem) = posted_sem {
                posting_sem.post();
            }

            // Wait until our semaphore is awakened
            waiting_sem.wait();

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
            } else if let Some(source) = state.global.create_cow.take() {
                let fd = match source {
                    CreateCowSource::Existing(cow_id) => {
                        state.local.pasture.get(&cow_id).unwrap().memfd
                    }
                    CreateCowSource::New(path, mode) => {
                        let cow_id = state.allocate_cow();
                        let inode = state.global.next_inode();
                        let current_time = state.global.current_time;
                        let uid = state.global.uid;
                        let gid = state.global.gid;

                        let file_info = std::rc::Rc::new_in(RefCell::new(FileInfo {
                            path: path.clone(),
                            cow: Some(cow_id),
                            dev_id: 0xfe01,
                            inode,
                            mode,
                            nlink: 1, // TODO: fix
                            backend: FileBackend::Feedback(FileFeedback { }),
                            uid,
                            gid,
                            atime: current_time,
                            btime: current_time,
                            mtime: current_time,
                            ctime: current_time,
                        }), fizzle_alloc());

                        if state.global.file_paths.insert(path.clone(), file_info).is_err() {
                            panic!("failed to add to file_paths")
                        }

                        let fd = state.local.pasture.get(&cow_id).unwrap().memfd;

                        copy_to_shmem(fd, &path);

                        fd
                    }
                };

                let cmsghdr = libc::cmsghdr {
                    cmsg_len: mem::size_of::<libc::cmsghdr>() + mem::size_of::<RawFd>(),
                    cmsg_level: libc::SCM_RIGHTS,
                    cmsg_type: libc::SOL_SOCKET,
                };

                let mut control = [0u8; mem::size_of::<libc::cmsghdr>() + mem::size_of::<RawFd>()];
                control[..mem::size_of::<libc::cmsghdr>()].copy_from_slice(unsafe { slice::from_ref(&cmsghdr).align_to::<u8>().1 });
                control[mem::size_of::<libc::cmsghdr>()..].copy_from_slice(&fd.to_ne_bytes());

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

                DelegationState::RunNextWorker

            } else if let Some((dst, raised_info)) = state.global.signal.take() {
                // ...because it has received a signal (e.g. from `kill()`, `pthread_kill()`)
                let signum = raised_info.signum();

                match dst {
                    SignalDestination::Process(pid) => {
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
                            // Now run that thread
                            DelegationState::SignalToThread(pid, chosen_thread, raised_info)

                        } else {
                            // None of the threads were ready--assign the (blocked) signal to one of the threads
                            'assigned: {
                                for siginfo in state.local.signals.values_mut() {
                                    if siginfo.raised[signum as usize - 1].is_none() {
                                        siginfo.raised[signum as usize - 1] = Some(raised_info);
                                        break 'assigned DelegationState::RunNextWorker;
                                    }
                                }

                                log::warn!(
                                    "Signal {} for PID {:?} was dropped",
                                    raised_info.signum(),
                                    pid,
                                );
                                DelegationState::RunNextWorker
                            }
                        }
                    }
                    SignalDestination::Thread(pid, thread_id) => {
                        // The worker raising the signal should always yield to the appropriate process
                        debug_assert_eq!(pid, state.local.process_info.borrow().pid);

                        let siginfo = state.local.signals.get_mut(&thread_id).unwrap();

                        if siginfo.blocked.contains(SignalSet::from_signum(signum)) {
                            // The signal was blocked--store it (if there's room)
                            if siginfo.raised[signum as usize - 1].is_none() {
                                siginfo.raised[signum as usize - 1] = Some(raised_info);
                            } else {
                                log::warn!(
                                    "Signal {} for TID {:?} was dropped",
                                    raised_info.signum(),
                                    thread_id,
                                );
                            }

                            DelegationState::RunNextWorker
                        } else if thread_id != thread::current().id() {
                            DelegationState::SignalToThread(pid, thread_id, raised_info)
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
        let mut polled_ready = Vec::new_in(fizzle_alloc());
        for endpoint_info in state.global.fuzz_endpoints.iter_mut() {
            endpoint_info.read_idx = 0;
            polled_ready
                .push(endpoint_info.read_polled.clone());
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
            .map(|plugin_info| plugin_info.module.clone())
            .collect();
        for module in modules {
            module.borrow_mut().fuzz_round_start(state.global.fuzz_input.as_slice());
        }

        // Gather all plugin endpoints
        let plugin_info_ids: Vec<_> = state
            .global
            .plugins
            .iter()
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
            write_buf.borrow_mut().clear();
            read_buf.borrow_mut().clear();
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
            log::debug!("added pending client with local addr {:?}", socket_info.borrow().local_addr);
            state.global.per_round_endpoints.push(socket_info);
        }

        drop(state);
    }

    fn remove_perround_endpoints(ctx: &mut FizzleSingleton) {
        let mut state = ctx.acquire();
        let global = &mut state.global;

        let endpoints: Vec<GlobalRc<SocketInfo>> = global.per_round_endpoints.drain(..).collect();
        for sock_info in endpoints {
            let local_transport = sock_info.borrow().local_transport();

            match &mut sock_info.borrow_mut().state {
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
                            ConnectedBackend::Plugin(plugin_info) => {
                                let read_polled = plugin_info.borrow().read_polled.clone();
                                let write_polled = plugin_info.borrow().write_polled.clone();
                                global.raise_polled(&read_polled);
                                global.raise_polled(&write_polled);
                            }
                            ConnectedBackend::Fuzz(fuzz_endpoint) => {
                                let read_polled = fuzz_endpoint.borrow().read_polled.clone();
                                global.raise_polled(&read_polled);
                            }
                            _ => unreachable!(),
                        }
                    }

                    if global
                        .per_round_clients
                        .push(PerRoundClientInfo {
                            source_address,
                            target_address,
                            backend: client_backend,
                        }).is_err() {
                            panic!("failed to insert to per_round_clients")
                        }
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
                state.global.fuzz_input.copy_from_slice(afl_buf);
            };

            state
                .global
                .fuzz_input
                .did_write(*crate::__afl_fuzz_len as usize);
        }

        #[cfg(not(feature = "pcr"))]
        loop {
            state.global.fuzz_input.reserve(16384);
            let current_len = state.global.fuzz_input.len();
            unsafe {
                let start = state.global.fuzz_input.as_mut_ptr().add(current_len);
                match libc::read(0, start.cast::<libc::c_void>(), 16384) {
                    ..=-1 => panic!("read() failed on stdin during PCR fuzzing"),
                    0 => break,
                    read_amount => {
                        state.global.fuzz_input.set_len(current_len + read_amount as usize);
                    }
                }
            }
        }

        state.global.time_fuzz_idx = 0;
    }

    fn send_signal(
        ctx: &mut FizzleSingleton,
        dst: SignalDestination,
        raised_info: RaisedSignalInfo,
    ) {
        let signum = raised_info.signum();

        // Save the current worker
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();
        let current_pid = state.local.process_info.borrow().pid;
        let dst_pid = match &dst {
            SignalDestination::Process(p) => *p,
            SignalDestination::Thread(p, _) => *p,
        };

        let disposition = state
            .global
            .pids
            .get(&dst_pid)
            .unwrap()
            .borrow()
            .signal_handlers[signum as usize - 1].clone();
        if let SigDisposition::Ignore = disposition {
            return; // Ignores the signal without saving it
        }

        // Once the signal has been received and handled, keep running the worker that sent it
        // this is Duration::ZERO because it must run first
        state
            .global
            .ready
            .push(ReadyItem {
                info: ReadyInfo::Worker(current_worker),
                timestamp: Duration::ZERO
            });

        // Add the signal to the global state
        assert!(state.global.signal.replace((dst.clone(), raised_info)).is_none());
        drop(state);

        // TODO: delegate to process/thread signal is being sent to
        if dst_pid == current_pid {
            match &dst {
                SignalDestination::Process(_) => {
                    Scheduler::handle_signal(ctx, raised_info);
                    // Te worker was pushed to the front of the queue, so we need to run it
                    Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);                   
                }
                SignalDestination::Thread(_, t) => {
                    if t == &thread::current().id() {
                        Scheduler::handle_signal(ctx, raised_info);
                        // Te worker was pushed to the front of the queue, so we need to run it
                        Scheduler::yield_worker(ctx, DelegationAction::RunNextWorker);
                    } else {
                        Scheduler::yield_worker(ctx, DelegationAction::RunThread(*t))
                    }
                }
            }
        } else {
            Scheduler::yield_worker(ctx, DelegationAction::RunProcess(dst_pid))
        }
    }

    fn handle_signal(ctx: &mut FizzleSingleton, raised_info: RaisedSignalInfo) {
        let mut state = ctx.acquire();
        let current_thread_id = thread::current().id();
        let current_pid = state.local.process_info.borrow().pid;

        let signum = raised_info.signum();

        let proc_siginfo = state.global.pids.get_mut(&current_pid).unwrap();
        let sig_handler = proc_siginfo.borrow().signal_handlers[signum as usize - 1].clone();

        if let RaisedSignalInfo::Timer(timer_info) = &raised_info {
            // Set itimer to repeat if applicable
            match timer_info.timer_id {
                libc::ITIMER_REAL => {
                    if let Some(real) = &state.local.itimer_real {
                        let pid = state.local.process_info.borrow().pid;
                        let current_time = state.global.current_time;
                        let interval = real.interval;
                        state.global.ready.push(ReadyItem {
                            timestamp: current_time.saturating_add(interval),
                            info: ReadyInfo::Timer(pid, TimerType::Real)
                        });
                    }
                }
                libc::ITIMER_VIRTUAL => {
                    if let Some(virt) = &state.local.itimer_virtual {
                        let pid = state.local.process_info.borrow().pid;
                        let current_time = state.global.current_time;
                        let interval = virt.interval;
                        state.global.ready.push(ReadyItem {
                            timestamp: current_time.saturating_add(interval),
                            info: ReadyInfo::Timer(pid, TimerType::Virtual)
                        });
                    }
                }
                libc::ITIMER_PROF => {
                    if let Some(prof) = &state.local.itimer_prof {
                        let pid = state.local.process_info.borrow().pid;
                        let current_time = state.global.current_time;
                        let interval = prof.interval;
                        state.global.ready.push(ReadyItem {
                            timestamp: current_time.saturating_add(interval),
                            info: ReadyInfo::Timer(pid, TimerType::Virtual)
                        });
                    }
                }
                _ => unreachable!("unknown itimer type"),
            }
        }

        let thread_siginfo = state.local.signals.get_mut(&current_thread_id).unwrap();

        match (
            &sig_handler,
            thread_siginfo
                .blocked
                .contains(SignalSet::from_signum(signum)),
        ) {
            (_, true) => {
                if thread_siginfo.raised[signum as usize - 1].is_some() {
                    // If there is already a pending signal, the incoming one is dropped
                    log::warn!(
                        "Signal {} for {:?}, {:?} dropped",
                        signum,
                        current_pid,
                        current_thread_id
                    );
                } else {
                    thread_siginfo.raised[signum as usize - 1] = Some(raised_info);
                    if thread_siginfo
                        .sigwait_set
                        .contains(SignalSet::from_signum(signum))
                    {
                        // A sigwait signal has become pending--awaken the waiting thread
                        thread_siginfo.sigwait_set = SignalSet::empty();
                        state.mark_thread_ready(current_thread_id);
                    }
                }
            }
            (SigDisposition::Default, false) => {
                if thread_siginfo.sigsuspend {
                    // Any call to `sigsuspend()` should return for this process
                    thread_siginfo.sigsuspend = false;
                    thread_siginfo.interrupted = true;
                    state.mark_thread_ready(current_thread_id);
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
                    state.mark_thread_ready(current_thread_id);
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
                    state.mark_thread_ready(current_thread_id);
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

        cmd.env("LD_PRELOAD", std::env::var("LD_PRELOAD").unwrap());
        cmd.env(FIZZLE_MEMORY_ENV, std::env::var(FIZZLE_MEMORY_ENV).unwrap());
        cmd.env(FIZZLE_ALLOC_ENV, std::env::var(FIZZLE_ALLOC_ENV).unwrap());
        cmd.spawn().unwrap();
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

        let mut state = ctx.acquire();

        // Clean up this thread's semaphore
        state.local.thread_locks.remove(&thread::current().id());

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
            let sem = state.local.thread_locks.get(&thread_id).unwrap().clone();
            drop(state);
            
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
        let pid = state.local.process_info.borrow().pid;
        let ppid = state.local.process_info.borrow().ppid;

        assert!(
            !(pid == Pid::PRIMARY),
            "main process forcibly terminated"
        );

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
        state
            .global
            .dead_pids
            .insert(pid, sigchild.clone());

        // If a parent is awaiting this process's death, notify it
        let awaiting = state
            .local.process_info.borrow_mut().awaiting_death.take();
        if let Some(awaiting_worker) = awaiting { 
            state.mark_worker_ready(awaiting_worker);
        }

        state.global.pids.remove(&pid);
        // TODO: mark process as able to be reaped

        // TODO: if a parent dies before a child does, the child will never be reaped.
        // Fix this...

        // TODO: other global cleanup (such as of socket state from dropped fds) here

        // Delegate execution to the primary process (it's guaranteed not to exit)
        let delegate_sem = state.global.pids.get(&Pid::PRIMARY).unwrap().borrow().semaphore.clone();

        drop(state);
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

    pub fn create_cow(ctx: &mut FizzleSingleton, source: CreateCowSource) {
        let mut state = ctx.acquire();
        let current_worker = state.current_worker();
        let current_pid = state.local.process_info.borrow().pid;

        if current_pid == Pid::PRIMARY {
            // No need to use `SCM_RIGHTS`--we're doing everything in the same process

            match source {
                CreateCowSource::Existing(_cow_id) => (), // Already created
                CreateCowSource::New(path, mode) => {
                    // Create a CoW
                    let cow_id = state.allocate_cow();

                    let inode = state.global.next_inode();
                    let current_time = state.global.current_time;
                    let uid = state.global.uid;
                    let gid = state.global.gid;

                    if !state.global.file_paths.contains_key(&path) {
                        let file_info = std::rc::Rc::new_in(RefCell::new(FileInfo {
                            path: path.clone(),
                            cow: Some(cow_id),
                            dev_id: 0xfe01,
                            inode,
                            mode,
                            nlink: 1, // TODO: fix
                            backend: FileBackend::Feedback(FileFeedback { }),
                            uid,
                            gid,
                            atime: current_time,
                            btime: current_time,
                            mtime: current_time,
                            ctime: current_time,
                        }), fizzle_alloc());

                        if state.global.file_paths.insert(path.clone(), file_info).is_err() {
                            panic!("failed to insert to file_paths")
                        }
                    } else {
                        let file_info = state.global.file_paths.get(&path).unwrap().clone();
                        file_info.borrow_mut().cow = Some(cow_id);
                    }

                    let memfd = state.local.pasture.get(&cow_id).unwrap().memfd;
                    copy_to_shmem(memfd, &path);
                }
            }

        } else {
            state.global.create_cow = Some(source.clone());

            state.global.ready.push(ReadyItem {
                info: ReadyInfo::Worker(current_worker),
                timestamp: Duration::ZERO, // Run immediately after this
            });

            let primary_sem = state.global.pids.get(&Pid::PRIMARY).unwrap().borrow().semaphore.clone();
            let current_sem = state.local.process_info.borrow().semaphore.clone();
            drop(state);

            // TODO: is this safe to do, or do we need to drop to yield_process?
            primary_sem.post();
            current_sem.wait();

            // Now the SCM_RIGHTS have been sent by the main process--receive and assign
            let mut state = ctx.acquire();

            let cow_id = match source {
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
                assert_eq!(libc::recvmsg(state.global.unix_read_fd, ptr::addr_of_mut!(msghdr), 0), 0);
            }

            let msg_len = msghdr.msg_controllen;
            let mut msg_idx = 0;

            while msg_len - msg_idx > mem::size_of::<libc::cmsghdr>() {
                let (s1, m, _s2) = unsafe { msg[msg_idx..].align_to::<libc::cmsghdr>() };
                assert!(s1.is_empty());
                let hdr = &m[0];
                if hdr.cmsg_len > msg_len {
                    break
                }

                if hdr.cmsg_type == libc::SOL_SOCKET && hdr.cmsg_level == libc::SCM_RIGHTS {
                    let msg_data = &msg[msg_idx + mem::size_of::<libc::cmsghdr>()..msg_idx + hdr.cmsg_len];
                    let (s1, fds, s2) = unsafe { msg_data.align_to::<RawFd>() };
                    assert!(s1.is_empty() && s2.is_empty() && fds.len() == 1);

                    state.local.pasture.insert(cow_id, CowInfo {
                        memfd: fds[0],
                    });

                    return
                }

                // Update msg index
                msg_idx = cmp::max(
                    CMSG_ALIGN(msg_idx + hdr.cmsg_len),
                    CMSG_ALIGN(msg_idx + mem::size_of::<libc::cmsghdr>())
                );

                if msg_idx > msg_len {
                    break
                }
            }

            unreachable!("SCM_RIGHTS not received on Unix socket")
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
    PauseCurrentWorker(DelegationSource, Option<std::rc::Rc<Semaphore, &'static TlsfHeap>>),
    RunNextWorker,
    RunThread(ThreadId),
    RunProcess(Pid),
}

pub enum DelegationState {
    PauseCurrentWorker(DelegationSource, Option<std::rc::Rc<Semaphore, &'static TlsfHeap>>),
    RunNextWorker,
    NoMoreWorkers,
    RunCurrentWorker,
    RunThread(ThreadId),
    RunProcess(Pid),
    RunPlugins,
    TerminateThread(TerminationMethod),
    SignalToThread(Pid, ThreadId, RaisedSignalInfo),
    HandleSignal(RaisedSignalInfo),
}

impl<'a> From<DelegationAction> for DelegationState {
    #[inline]
    fn from(value: DelegationAction) -> Self {
        match value {
            DelegationAction::PauseCurrentWorker(src, sem) => Self::PauseCurrentWorker(src, sem),
            DelegationAction::RunNextWorker => Self::RunNextWorker,
            DelegationAction::RunThread(t) => Self::RunThread(t),
            DelegationAction::RunProcess(p) => Self::RunProcess(p),
        }
    }
}

#[derive(Clone)]
pub enum DelegationSource {
    Thread(std::rc::Rc<Semaphore, &'static TlsfHeap>),
    Process(std::rc::Rc<Semaphore, &'static TlsfHeap>),
}

#[derive(Clone, Debug)]
pub enum TerminationMethod {
    Cancellation,
    ProcessExit(i32),
    ProcessImmediateExit(i32),
    ThreadExit(*mut libc::c_void),
    Signal(RaisedSignalInfo),
}
