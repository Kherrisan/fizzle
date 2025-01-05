use std::array;
use std::ffi::CString;
use std::{io::Error, mem::MaybeUninit, ptr, thread};

use bitflags::bitflags;

use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;
use heapless::IndexSet;

use crate::arena::{ArenaKey, Rc};
use crate::constants::*;
use crate::errno::Errno;
use crate::handlers::signal::SigDisposition;
use crate::scheduler::{DelegationSource, Event, Outcome, TerminationMethod};
use crate::semaphore::Semaphore;
use crate::state::{fizzle_singleton, set_entered_handler, FizzleState, InheritedState};

use super::descriptor::{DescriptorId, FdResource};
use super::id::{WorkerId, WorkerInfo};
use super::signal::*;
use super::thread::ThreadInfo;

pub type AtExitFunction = unsafe extern "C" fn();
pub type OnExitFunction = unsafe extern "C" fn(libc::c_int, *mut libc::c_void);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
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
    type Value = ProcessInfo;
}

impl ProcessId {
    /// The PID corresponding to the main process (2).
    pub fn main_process() -> Self {
        Self(0)
    }

    pub fn is_main_process(&self) -> bool {
        self == &Self::main_process()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ProcessGroupId(usize);

impl ProcessGroupId {
    pub fn from_worker(worker_id: &WorkerId) -> Self {
        Self(worker_id.as_id() as usize)
    }

    pub fn from_pgid(pgid: libc::c_int) -> Self {
        assert!(pgid > 0);
        Self(pgid as usize)
    }

    pub fn as_pgid(&self) -> libc::c_int {
        self.0 as libc::c_int
    }
}

impl ArenaKey for ProcessGroupId {
    type Value = heapless::FnvIndexSet<ProcessId, FIZZLE_MAX_PROCESS_GROUP_SIZE>;
}

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    /// The PID of this process (e.g., the TID of the main thread).
    pub pid: WorkerId,
    /// The parent process ID corresponding to the given process.
    pub ppid: WorkerId,
    /// The process group ID corresponding to the given process.
    pub pgid: ProcessGroupId,
    /*
    /// Signals that have been raised for the process via `kill` (or some other natural event) but
    /// that cannot be immediately handled as they are blocked.
    pub raised_signals: RaisedSignalSet,
    */
    /// The handler to be run when the signal is received.
    ///
    /// According to `pthreads(7)`, POSIX.1 specifies that threads of a process should all share
    /// signal disposition; this disposition is indicated by the `SigCallback` enum.
    pub signal_handlers: SignalHandlers,
    /// The set of child processes for the given process.
    pub children: heapless::FnvIndexSet<ProcessId, FIZZLE_MAX_CHILD_PROCESSES>,
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

                let parent_process_id = state.local.process_id;

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

                        let parent_info = state.global.processes.get(&parent_process_id).unwrap();
                        let ppid = parent_info.pid;
                        let pgid = parent_info.pgid;
                        let signal_handlers = parent_info.signal_handlers.clone();

                        // Assign a pid to this process--use parent ProcessId TEMPORARILY until child id assigned
                        let mut pid = state
                            .global
                            .ids
                            .allocate(WorkerInfo::current(parent_process_id))
                            .unwrap();
                        Rc::upref(&mut pid);
                        let pid = *pid;

                        // The child process needs a unique identifier from the parent
                        let mut child_process_id = state
                            .global
                            .processes
                            .allocate(ProcessInfo {
                                pid,
                                ppid,
                                pgid,
                                signal_handlers,
                                children: IndexSet::default(),
                            })
                            .unwrap();
                        Rc::upref(&mut child_process_id);
                        let child_process_id = *child_process_id;

                        // Now fix the child ProcessId;
                        state.global.ids.get_mut(&pid).unwrap().process_id = child_process_id;

                        // Add the child process to the parent process's child list
                        state
                            .global
                            .processes
                            .get_mut(&parent_process_id)
                            .unwrap()
                            .children
                            .insert(child_process_id)
                            .unwrap();

                        // Add the child process to the parent process's group
                        state
                            .global
                            .process_groups
                            .get_mut(&pgid)
                            .unwrap()
                            .insert(child_process_id)
                            .unwrap();

                        // Assign the child a unique process ID
                        state.local.process_id = child_process_id;

                        // Initialize the child process's global lock
                        let sem_opt =
                            &mut state.global.process_locks[usize::from(child_process_id)];
                        unsafe {
                            // TODO: this seems to behave safely, but check with Miri
                            let uninit_sem = (*(ptr::from_mut(sem_opt)
                                as *mut Option<MaybeUninit<Semaphore>>))
                                .insert(MaybeUninit::uninit());
                            Semaphore::initialize(uninit_sem, true, 0);
                        }

                        // Remove any state from main process
                        state.local.main_state = None;

                        // `fork()` only copies the current thread
                        state.local.pthreads.clear();
                        state.local.pthreads.insert(
                            unsafe { libc::pthread_self() },
                            ThreadInfo::new(thread::current().id(), false, true),
                        );

                        // Remove any pending signals
                        state
                            .local
                            .signals
                            .get_mut(&thread::current().id())
                            .unwrap()
                            .raised = array::from_fn(|_| None);

                        // Remove all pthread cleanup routines except for the current thread's
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

                        // Upref the resources each file descriptor points towards

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

                        self.state = ProcessForkState::RunPostHandlers(child_handlers, pid.as_id());
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

        let process_id = state.local.process_id;
        let fds = state.local.fds.as_ref().clone();
        let signal_handlers = state
            .global
            .processes
            .get(&process_id)
            .unwrap()
            .signal_handlers
            .clone();
        let sigmask = state
            .local
            .signals
            .get(&thread::current().id())
            .unwrap()
            .blocked;

        state.global.inherited_state = Some(InheritedState {
            process_id,
            fds,
            signal_handlers,
            sigmask,
        });

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
                        let FdResource::File(open_file_id) = &fd_info.resource else {
                            state.global.inherited_state = None;
                            return Outcome::Error(Errno::EINVAL);
                        };

                        // TODO: must be opened read-only with O_PATH set and execute permissions

                        let open_file = state.global.open_files.get(&open_file_id).unwrap();
                        let path = &state.global.files.get(&open_file.file).unwrap().path;

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
            ExecLocation::AtDirectory(_fd, _path) => {
                todo!("execveat not implemented") // TODO: implement
            }
        }

        state.global.inherited_state = None;

        Outcome::Error(Errno::get_errno())
    }
}

pub struct ProcessExitEvent {
    status: libc::c_int,
    run_cleanup: bool,
}

impl ProcessExitEvent {
    pub fn new(status: libc::c_int, run_cleanup: bool) -> Self {
        Self {
            status,
            run_cleanup,
        }
    }
}

impl Event for ProcessExitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.run_cleanup {
            true => Outcome::TerminateProcess(TerminationMethod::ProcessExit(self.status)),
            false => {
                Outcome::TerminateProcess(TerminationMethod::ProcessImmediateExit(self.status))
            }
        }
    }
}

pub struct ProcessAtExitEvent {
    handler: AtExitFunction,
}

impl ProcessAtExitEvent {
    pub fn new(handler: AtExitFunction) -> Self {
        Self { handler }
    }
}

impl Event for ProcessAtExitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state.local.atexit_handlers.push(self.handler);
        Outcome::Success(())
    }
}

pub struct ProcessOnExitEvent {
    handler: OnExitFunction,
    arg: *mut libc::c_void,
}

impl ProcessOnExitEvent {
    pub fn new(handler: OnExitFunction, arg: *mut libc::c_void) -> Self {
        Self { handler, arg }
    }
}

impl Event for ProcessOnExitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state.local.on_exit_handlers.push((self.handler, self.arg));
        Outcome::Success(())
    }
}

pub enum WaitType {
    AllChildren,
    Pid(ProcessId),
    PidFd(DescriptorId),
    Gid(ProcessGroupId),
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct WaitOptions: libc::c_int {
        /// Return immediately if not child has exited.
        const NO_HANG = libc::WNOHANG;
        /// Return if a child has stopped, but is not traced via ptrace (`ptrace`d children always return).
        const UNTRACED = libc::WUNTRACED;
        /// Wait for (previously stopped) children that have been resumed.
        const CONTINUED = libc::WCONTINUED;
        /// Wait for children that have exited
        const EXITED = libc::WEXITED;
        /// Wait for children that have been stopped by delivery of a signal.
        const STOPPED = libc::WSTOPPED;
    }
}

pub enum ProcessWaitState {
    Start,
    Finish,
}

pub struct ProcessWaitEvent {
    wait_type: WaitType,
    options: WaitOptions,
    state: ProcessWaitState,
}

impl ProcessWaitEvent {
    pub fn new(wait_type: WaitType, options: WaitOptions) -> Self {
        Self {
            wait_type,
            options,
            state: ProcessWaitState::Start,
        }
    }
}

impl Event for ProcessWaitEvent {
    type Success = Option<SigChildInfo>;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let process_id = state.local.process_id;

        match self.state {
            ProcessWaitState::Start => match self.wait_type {
                WaitType::AllChildren => {
                    let children: Vec<_> = state
                        .global
                        .processes
                        .get(&process_id)
                        .unwrap()
                        .children
                        .iter()
                        .collect();
                    if children.is_empty() {
                        return Outcome::Success(None);
                    }

                    for child in children {
                        state
                            .global
                            .awaiting_process_death
                            .get_mut(&child)
                            .unwrap()
                            .insert(WorkerInfo::current(process_id))
                            .unwrap();
                    }

                    self.state = ProcessWaitState::Finish;
                    Outcome::Yield(None)
                }
                WaitType::Pid(child_process) => {
                    if child_process.0 == 0 {
                        return Outcome::Error(Errno::ESRCH);
                    }

                    // Check to see if the PID is a child of this process
                    if !state
                        .global
                        .processes
                        .get(&process_id)
                        .unwrap()
                        .children
                        .contains(&child_process)
                    {
                        return Outcome::Error(Errno::ECHILD);
                    }

                    // Check to see if SIGCHLD is blocked in this process
                    if state
                        .global
                        .processes
                        .get(&process_id)
                        .unwrap()
                        .signal_handlers[libc::SIGCHLD as usize]
                        == SigDisposition::Ignore
                    {
                        return Outcome::Error(Errno::ECHILD);
                    }

                    if self.options.intersects(
                        WaitOptions::CONTINUED | WaitOptions::STOPPED | WaitOptions::UNTRACED,
                    ) {
                        unimplemented!("wait() on continued or stopped children");
                    }

                    if let Some(exited_info) = state.global.exited_processes.get(&child_process) {
                        return Outcome::Success(Some(*exited_info));
                    }

                    if self.options.contains(WaitOptions::NO_HANG) {
                        return Outcome::Success(None);
                    }

                    // Suspend execution until the child has exited
                    state
                        .global
                        .awaiting_process_death
                        .get_mut(&child_process)
                        .unwrap()
                        .insert(WorkerInfo::current(process_id))
                        .unwrap();
                    self.state = ProcessWaitState::Finish;
                    Outcome::Yield(None)
                }
                WaitType::Gid(group_id) => {
                    let mut children = Vec::new();

                    let Some(group_info) = state.global.process_groups.get(&group_id) else {
                        return Outcome::Success(None);
                    };

                    for child_process_id in group_info.iter() {
                        if state
                            .global
                            .processes
                            .get(&process_id)
                            .unwrap()
                            .children
                            .contains(child_process_id)
                        {
                            children.push(child_process_id);
                        }
                    }

                    if children.is_empty() {
                        return Outcome::Success(None);
                    }

                    for child in children {
                        state
                            .global
                            .awaiting_process_death
                            .get_mut(&child)
                            .unwrap()
                            .insert(WorkerInfo::current(process_id))
                            .unwrap();
                    }

                    self.state = ProcessWaitState::Finish;
                    Outcome::Yield(None)
                }
                WaitType::PidFd(_) => todo!("pid fd not implemented"),
            },
            ProcessWaitState::Finish => {
                let children: Vec<_> = state
                    .global
                    .processes
                    .get(&process_id)
                    .unwrap()
                    .children
                    .iter()
                    .collect();

                let mut awaiting = Vec::new();

                let current_worker = WorkerInfo::current(process_id);
                for child in children {
                    if state
                        .global
                        .awaiting_process_death
                        .get_mut(&child)
                        .unwrap()
                        .remove(&current_worker)
                    {
                        awaiting.push(*child);
                    }
                }

                for child in awaiting {
                    if let Some(proc_wait_info) = state.global.exited_processes.get(&child) {
                        return Outcome::Success(Some(*proc_wait_info));
                    }
                }

                unreachable!(
                    "No child process was ready to be reaped, yet `waitpid()` was awakened"
                );
            }
        }
    }
}
