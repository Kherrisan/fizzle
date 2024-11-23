use std::array;
use std::ffi::CString;
use std::{io::Error, mem::MaybeUninit, ptr, thread};

use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;

use crate::arena::ArenaKey;
use crate::errno::Errno;
use crate::scheduler::{DelegationSource, Event, Outcome};
use crate::semaphore::Semaphore;
use crate::state::{fizzle_singleton, set_entered_handler, FizzleState};

use super::descriptor::{DescriptorId, FdResource};
use super::signal::ProcSigInfo;
use super::thread::ThreadInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId(usize);

impl From<ProcessId> for usize {
    fn from(value: ProcessId) -> Self {
        value.0
    }
}

impl From<usize> for ProcessId {
    fn from(value: usize) -> Self {
        ProcessId(value)
    }
}

impl ArenaKey for ProcessId {
    type Value = ProcSigInfo;
}

impl ProcessId {
    pub fn main_process() -> Self {
        Self(0)
    }

    pub fn is_main_process(&self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Debug)]
pub struct AtForkInfo {
    pub prepare: Option<unsafe extern "C" fn()>,
    pub parent: Option<unsafe extern "C" fn()>,
    pub child: Option<unsafe extern "C" fn()>,
}

pub enum ProcessForkState {
    Start,
    RunPreHandlers(Vec<Option<unsafe extern "C" fn()>>),
    RunFork,
    RunPostHandlers(Vec<Option<unsafe extern "C" fn()>>, libc::pid_t),
    Finish(libc::pid_t),
}

pub struct ProcessForkEvent {
    state: ProcessForkState,
}

impl ProcessForkEvent {
    pub fn new() -> Self {
        Self {
            state: ProcessForkState::Start,
        }
    }
}

impl Event for ProcessForkEvent {
    type Success = libc::pid_t;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            ProcessForkState::Start => {
                // Run pthread_atfork handlers (in LIFO order)
                let pre_handlers: Vec<_> = state
                    .local
                    .atfork_handlers
                    .iter()
                    .map(|i| i.prepare)
                    .collect();
                self.state = ProcessForkState::RunPreHandlers(pre_handlers);
                Outcome::Continue
            }
            ProcessForkState::RunPreHandlers(v) => {
                while let Some(f) = v.pop() {
                    match f {
                        None => continue,
                        Some(f) => return Outcome::Execute(f),
                    }
                }

                self.state = ProcessForkState::RunFork;
                Outcome::Continue
            }
            ProcessForkState::RunFork => {
                // Initialize AFL (forkservers and multi-process applications don't play well)
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
                    log::debug!("__afl_manual_init finished");
                }

                // The child process needs a unique identifier from the parent
                let child_process_id = state.global.assign_process_id();

                /*
                // Generally, moving `KeyedArena`s is unsafe because `Rc<>` references rely on arenas
                // remaining in a fixed location in memory. However, `fds` never makes use of these
                // references, so this is safe to do.
                state.global.transfer_fds = Some((*state.local.fds).clone());
                */

                // Run pthread_atfork child handlers (in FIFO order)
                // SAFETY: this must run before `fork()` to uphold noalias
                let parent_handlers: Vec<_> = state
                    .local
                    .atfork_handlers
                    .iter()
                    .map(|i| i.parent)
                    .collect();

                // Let the scheduler know we have more to execute once the new thread is done.
                state.mark_thread_ready(thread::current().id());

                let pid = unsafe { libc::fork() };

                // SAFETY: parent process must not use `state` after here

                match pid {
                    // The child process initializes itself
                    0 => {
                        // This *technically* shouldn't be needed since we share memory with the
                        // parent, but just to be safe...
                        set_entered_handler(true);

                        // Clean up child processes if the parent is ever killed
                        unsafe {
                            assert_eq!(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM), 0);
                        }

                        // SAFETY: first time this is called in this process; parent process won't
                        // access the fizzle singleton until it is awakened.
                        let mut ctx = unsafe { fizzle_singleton() };
                        let mut state = ctx.acquire();

                        // Make our process ID unique
                        state.local.process_id = child_process_id;

                        // Initialize this process's global lock
                        let sem_opt =
                            &mut state.global.process_locks[usize::from(child_process_id)];
                        unsafe {
                            // TODO: this seems to behave safely, but check with Miri
                            let uninit_sem = (*(ptr::from_mut(sem_opt)
                                as *mut Option<MaybeUninit<Semaphore>>))
                                .insert(MaybeUninit::uninit());
                            Semaphore::initialize(uninit_sem, true, 0);
                        }

                        state.local.plugins = None;

                        // `fork()` only copies the current thread
                        state.local.pthreads.clear();
                        state.local.pthreads.insert(
                            unsafe { libc::pthread_self() },
                            ThreadInfo::new(thread::current().id(), false, true),
                        );

                        let cleanup = state
                            .local
                            .pthread_cleanup
                            .remove(&thread::current().id())
                            .unwrap();
                        state.local.pthread_cleanup.clear();
                        state
                            .local
                            .pthread_cleanup
                            .insert(thread::current().id(), cleanup);

                        // The current (forking) thread isn't terminating
                        state.local.terminated_threads.clear();
                        state.local.awaiting_thread_death.clear();

                        // TODO: upref the resources each file descriptor points towards
                        /*
                        let resources: Vec<_> = state.local.fds.values().map(|fd_info| fd_info.resource.clone()).collect();

                        for resource in resources {
                            match resource {
                                FdResource::Directory(rc) => (),
                                FdResource::Epoll(rc) => (),
                                FdResource::EventFd(rc) => todo!(),
                                FdResource::File(rc) => todo!(),
                                FdResource::MessageQueue(rc) => todo!(),
                                FdResource::Pipe(rc) => todo!(),
                                FdResource::Stdin => todo!(),
                                FdResource::Stdout => todo!(),
                                FdResource::Stderr => todo!(),
                                FdResource::Socket(rc) => todo!(),
                            }
                        }
                        */

                        // From the man pages:

                        // TODO: Wipe process-local thread information (except for this thread)
                        // TODO: Clear all pending signals
                        // TODO: Remove any semaphore adjustments (e.g. from `semop`)
                        // TODO: remove any `fcntl` record locks
                        // TODO: remove any timers (`alarm()`, `setitimer()`, etc.)
                        // TODO: remove any outstanding asynchronous I/O operations (aio_read, aio_write)
                        // TODO: remove any dnotify notifications (see F_NOTIFY in fcntl)
                        // TODO: remove PR_SET_PDEATHSIG prctl
                        // TODO: Set default timer slack value to the parent's current timer slack value (PR_SET_TIMERSLACK in prctl)
                        // TODO: set termination signal of child to SIGCHLD
                        // TODO: after a fork() in a multithreaded program, the child can safely call only async-signal-safe functions until execve

                        let child_handlers: Vec<_> = state
                            .local
                            .atfork_handlers
                            .iter()
                            .rev()
                            .map(|i| i.child)
                            .collect();
                        state.local.atfork_handlers.clear();

                        self.state = ProcessForkState::RunPostHandlers(child_handlers, pid);
                        Outcome::Continue
                    }
                    // The parent process pauses for the child to run
                    1.. => {
                        self.state = ProcessForkState::RunPostHandlers(parent_handlers, pid);
                        Outcome::Pause(DelegationSource::Process)
                    }
                    // Process creation failed--should be seen as fatal within fuzzing context
                    ..=-1 => panic!(
                        "`fork()` process creation failed ({:?})",
                        Error::last_os_error()
                    ),
                }
            }
            ProcessForkState::RunPostHandlers(v, pid) => {
                while let Some(f) = v.pop() {
                    match f {
                        None => continue,
                        Some(f) => return Outcome::Execute(f),
                    }
                }

                self.state = ProcessForkState::Finish(*pid);
                Outcome::Continue
            }
            ProcessForkState::Finish(pid) => Outcome::Success(*pid),
        }
    }
}

pub struct RegisterAtForkEvent {
    info: AtForkInfo,
}

impl RegisterAtForkEvent {
    pub fn new(info: AtForkInfo) -> Self {
        Self { info }
    }
}

impl Event for RegisterAtForkEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state.local.atfork_handlers.push(self.info.clone());
        Outcome::Success(())
    }
}

pub enum ExecLocation {
    File(FilePath<MAX_PATH_LEN>),
    ShellFile(FilePath<MAX_PATH_LEN>),
    Descriptor(DescriptorId),
    AtDirectory(DescriptorId, FilePath<MAX_PATH_LEN>),
}

pub struct ProcessExecEvent {
    cmd_location: ExecLocation,
    env: Option<Vec<CString>>,
    args: Vec<CString>,
}

impl ProcessExecEvent {
    pub fn new(cmd_location: ExecLocation, env: Option<Vec<CString>>, args: Vec<CString>) -> Self {
        Self {
            cmd_location,
            env,
            args,
        }
    }
}

impl Event for ProcessExecEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: cleanup here

        let argp: [*const libc::c_char; 128] = array::from_fn(|i| {
            if i < self.args.len() {
                self.args[i].as_ptr()
            } else {
                ptr::null()
            }
        });

        match &self.cmd_location {
            ExecLocation::File(f) => {
                let cmd = f.data().as_ptr() as *const libc::c_char;
                match &self.env {
                    Some(e) => {
                        let envp: [*const libc::c_char; 128] = array::from_fn(|i| {
                            if i < e.len() {
                                e[i].as_ptr()
                            } else {
                                ptr::null()
                            }
                        });

                        unsafe {
                            libc::execve(cmd, argp.as_ptr(), envp.as_ptr());
                        }
                    }
                    None => unsafe {
                        libc::execv(cmd, argp.as_ptr());
                    },
                }
            }
            ExecLocation::ShellFile(f) => {
                let cmd = f.data().as_ptr() as *const libc::c_char;
                match &self.env {
                    Some(e) => {
                        let envp: [*const libc::c_char; 128] = array::from_fn(|i| {
                            if i < e.len() {
                                e[i].as_ptr()
                            } else {
                                ptr::null()
                            }
                        });

                        unsafe {
                            libc::execvpe(cmd, argp.as_ptr(), envp.as_ptr());
                        }
                    }
                    None => unsafe {
                        libc::execvp(cmd, argp.as_ptr());
                    },
                }
            }
            ExecLocation::Descriptor(descriptor) => {
                let envp: [*const libc::c_char; 128] = self
                    .env
                    .as_ref()
                    .map(|e| {
                        array::from_fn(|i| {
                            if i < e.len() {
                                e[i].as_ptr()
                            } else {
                                ptr::null()
                            }
                        })
                    })
                    .unwrap();

                match state.local.fds.get(&descriptor) {
                    Some(fd_info) => {
                        let FdResource::File(file_id) = &fd_info.resource else {
                            return Outcome::Error(Errno::EINVAL);
                        };

                        // TODO: must be opened read-only with O_PATH set and execute permissions

                        let file_id = file_id.clone();
                        let path = &state.global.files.get(&file_id).unwrap().path;

                        let cmd = path.data().as_ptr() as *const libc::c_char;

                        unsafe {
                            libc::execve(cmd, argp.as_ptr(), envp.as_ptr());
                        }
                    }
                    None => unsafe {
                        log::warn!("fexecve called on unknown file descriptor--passing through...");
                        libc::fexecve(descriptor.as_raw_fd(), argp.as_ptr(), envp.as_ptr());
                    },
                }
            }
            ExecLocation::AtDirectory(fd, path) => {
                todo!() // TODO: implement
            }
        }

        Outcome::Error(Errno::get_errno())
    }
}

pub struct ProcessExitEvent {
    run_cleanup: bool,
}

impl ProcessExitEvent {
    pub fn new(run_cleanup: bool) -> Self {
        Self { run_cleanup }
    }
}

impl Event for ProcessExitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        Outcome::TerminateThread(TerminationMethod::ThreadExit(self.retval))
    }
}
