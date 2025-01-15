use std::array;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::CString;
use std::{io::Error, ptr, thread};

use bitflags::bitflags;

use embedded_alloc::TlsfHeap;
use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;

use crate::GlobalSet;
use crate::errno::Errno;
use crate::handlers::signal::SigDisposition;
use crate::scheduler::{fizzle_alloc, fizzle_singleton, DelegationSource, Event, Outcome, TerminationMethod};
use crate::semaphore::Semaphore;
use crate::state::{set_entered_handler, FizzleState, InheritedState};

use super::descriptor::{Descriptor, FdResource};
use super::id::Worker;
use super::signal::*;
use super::thread::ThreadInfo;

pub type AtExitFunction = unsafe extern "C" fn();
pub type OnExitFunction = unsafe extern "C" fn(libc::c_int, *mut libc::c_void);

/*
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId(usize);

impl ProcessId {
    pub const ZERO: ProcessId = ProcessId(0);

    pub fn next(id: &ProcessId) -> Self {
        Self(id.0 + 1)
    }

    /// The PID corresponding to the main process (2).
    pub fn main_process() -> Self {
        Self(2)
    }

    pub fn is_main_process(&self) -> bool {
        self == &Self::main_process()
    }
}
*/

/// The ID associated with a given process.
///
/// Equivalent to a pid_t.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pid(usize);

impl Pid {
    /// The PID corresponding to the `init` process (e.g., PID 1).
    pub const INIT: Pid = Pid(1);
    /// The PID corresponding to the primary process (e.g., PID 2).
    pub const PRIMARY: Pid = Pid(2);

    pub fn next(&self) -> Self {
        Self(self.0 + 1)
    }

    pub fn from_raw(tid: libc::pid_t) -> Self {
        Self(tid.try_into().unwrap())
    }

    pub fn as_raw(&self) -> libc::pid_t {
        self.0 as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pgid(usize);

impl Pgid {
    pub fn from_raw(pgid: libc::pid_t) -> Self {
        assert!(pgid > 0);
        Self(pgid as usize)
    }

    pub fn as_raw(&self) -> libc::pid_t {
        self.0 as libc::pid_t
    }

    pub fn from_pid(pid: Pid) -> Self {
        Self(pid.0)
    }
}

pub struct ProcessInfo {
    /// The global semaphore used to awaken this process.
    pub semaphore: std::rc::Rc<Semaphore, &'static TlsfHeap>,
    /// Threads that are awaiting the death of this process (e.g. via `waitpid`).
    pub awaiting_death: Option<Worker>,
    /// The PID of this process (e.g., the TID of the main thread).
    pub pid: Pid,
    /// The parent process ID corresponding to the given process.
    pub ppid: Pid,
    /// The process group ID corresponding to the given process.
    pub pgid: Pgid,
    /// The handler to be run when the signal is received.
    ///
    /// According to `pthreads(7)`, POSIX.1 specifies that threads of a process should all share
    /// signal disposition; this disposition is indicated by the `SigCallback` enum.
    pub signal_handlers: SignalHandlers,
    /// The set of child processes for the given process.
    pub children: GlobalSet<Pid>,
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

                let parent_info = state.local.process_info.clone();

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
                        // SAFETY: this is a forked process, so it already has its FizzleState initialized.
                        let mut state = ctx.acquire();

                        let pid = state.global.next_pid();
                        let ppid = parent_info.borrow().pid;
                        let pgid = parent_info.borrow().pgid;
                        let signal_handlers = parent_info.borrow().signal_handlers.clone();

                        state.local.process_info = std::rc::Rc::new_in(RefCell::new(ProcessInfo {
                            pid,
                            ppid,
                            pgid,
                            semaphore: Semaphore::new_rc_in(0, true, fizzle_alloc()),
                            signal_handlers,
                            awaiting_death: None,
                            children: BTreeSet::new_in(fizzle_alloc()),
                        }), fizzle_alloc());

                        let process_info = state.local.process_info.clone();
                        state.global.pids.insert(pid, process_info);

                        // Add the child process to the parent process's child list
                        parent_info.borrow_mut().children.insert(pid);

                        // Add the child process to the parent process's group
                        state
                            .global
                            .process_groups
                            .get_mut(&pgid)
                            .unwrap()
                            .insert(pid);

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

                        self.state = ProcessForkState::RunPostHandlers(child_handlers, pid.as_raw());
                        Outcome::Continue
                    }
                    // The parent process pauses for the child to run
                    1.. => {
                        self.state = ProcessForkState::RunPostHandlers(parent_handlers, pid);
                        Outcome::Pause(DelegationSource::Process, None)
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
    Descriptor(Descriptor),
    AtDirectory(Descriptor, FilePath<MAX_PATH_LEN>),
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

        let process_info = state.local.process_info.clone();
        let pid = process_info.borrow().pid;
        let ppid = process_info.borrow().ppid;
        let pgid = process_info.borrow().pgid;
        let fds = state.local.fds.clone();
        let signal_handlers = process_info.borrow().signal_handlers.clone();
        
        let sigmask = state
            .local
            .signals
            .get(&thread::current().id())
            .unwrap()
            .blocked;

        state.global.inherited_state = Some(InheritedState {
            pid, // TODO: are these meant to be switched up to reflect child relationship?
            ppid,
            pgid,
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
                let cmd = f.data().as_ptr().cast::<libc::c_char>();
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
                let cmd = f.data().as_ptr().cast::<libc::c_char>();
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
                        let FdResource::File(open_file) = &fd_info.resource else {
                            state.global.inherited_state = None;
                            return Outcome::Error(Errno::EINVAL);
                        };

                        // TODO: must be opened read-only with O_PATH set and execute permissions

                        let path = open_file.borrow().file.borrow().path.clone();

                        let cmd = path.data().as_ptr().cast::<libc::c_char>();

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
    Pid(Pid),
    PidFd(Descriptor),
    Gid(Pgid),
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
        let current_worker = state.current_worker();
        let process_info = state.local.process_info.clone();


        match self.state {
            ProcessWaitState::Start => match self.wait_type {
                WaitType::AllChildren => {

                    let process_info_borrow = process_info.borrow();
                    let children: Vec<_> = process_info_borrow.children.iter().collect();
                    if children.is_empty() {
                        return Outcome::Success(None); // TODO: is this correct?
                    }

                    for child in children {
                        if let Some(child_info) = state.global.pids.get(child) {
                            child_info.borrow_mut().awaiting_death = Some(current_worker);
                        }
                    }

                    // TODO: what if one thread creates new children while another thread has already called `waitid()` on any children??

                    self.state = ProcessWaitState::Finish;
                    Outcome::Yield(None)
                }
                WaitType::Pid(child_pid) => {
                    // Check to see if the PID is a child of this process
                    if !state.local.process_info.borrow().children.contains(&child_pid) {
                        return Outcome::Error(Errno::ECHILD)
                    }

                    // Check to see if SIGCHLD is blocked in this process
                    if state.local.process_info.borrow().signal_handlers[libc::SIGCHLD as usize - 1] == SigDisposition::Ignore {
                        return Outcome::Error(Errno::ECHILD) // TODO: are these errors correct?
                    }

                    if self.options.intersects(
                        WaitOptions::CONTINUED | WaitOptions::STOPPED | WaitOptions::UNTRACED,
                    ) {
                        unimplemented!("wait() on continued or stopped children");
                    }

                    if let Some(exited_info) = state.global.dead_pids.remove(&child_pid) {
                        return Outcome::Success(Some(exited_info))
                    }

                    if self.options.contains(WaitOptions::NO_HANG) {
                        return Outcome::Success(None);
                    }

                    let current_worker = state.current_worker();
                    state.global.pids.get_mut(&child_pid).unwrap().borrow_mut().awaiting_death = Some(current_worker);

                    // Suspend execution until the child has exited
                    self.state = ProcessWaitState::Finish;
                    Outcome::Yield(None)
                }
                WaitType::Gid(group_id) => {
                    let mut children = Vec::new();

                    let Some(group_info) = state.global.process_groups.get(&group_id) else {
                        return Outcome::Success(None);
                    };

                    for child_process_id in group_info.iter() {
                        if state.local.process_info.borrow().children.contains(child_process_id) {
                            children.push(*child_process_id);
                        }
                    }

                    if children.is_empty() {
                        return Outcome::Success(None);
                    }

                    let worker = state.current_worker();
                    for child in children.iter() {
                        if let Some(child_info) = state.global.pids.get_mut(child) {
                            child_info.borrow_mut().awaiting_death = Some(worker.clone());
                        } else {
                            unreachable!()
                        }
                    }

                    self.state = ProcessWaitState::Finish;
                    Outcome::Yield(None)
                }
                WaitType::PidFd(fd) => todo!("pidfd for fd {} not implemented", fd.as_raw_fd()),
            },
            ProcessWaitState::Finish => {
                let proc_info_borrow = state.local.process_info.borrow();
                let children: Vec<_> = proc_info_borrow.children.iter().collect();
                let mut awaiting = Vec::new();

                for child in children {
                    if let Some(child_info) = state.global.pids.get_mut(child) {
                        if let Some(worker) = &child_info.borrow_mut().awaiting_death {
                            if &current_worker == worker {
                                awaiting.push(*child);
                            }
                        }
                    }
                }

                for child in awaiting {
                    if let Some(proc_wait_info) = state.global.dead_pids.remove(&child) {
                        return Outcome::Success(Some(proc_wait_info))
                    }
                }

                // TODO: put EINTR here?
                unreachable!(
                    "No child process was ready to be reaped, yet `waitpid()` was awakened"
                );
            }
        }
    }
}
