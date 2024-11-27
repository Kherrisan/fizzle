use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::time::Duration;
use std::{array, mem, ptr, thread};

use bitflags::bitflags;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::{FizzleState, SignalDestination};

use super::id::{WorkerId, WorkerInfo};
use super::process::ProcessGroupId;

pub type SignalHandlers = [SigDisposition; 32];
pub type RaisedSignalSet = [Option<RaisedSignalInfo>; 32];

pub const SI_USER: libc::c_int = 0;
pub const SI_QUEUE: libc::c_int = -1;
pub const SI_TIMER: libc::c_int = -2;
pub const SI_MESGQ: libc::c_int = -3;
pub const SI_ASYNCIO: libc::c_int = -4;
pub const SI_TKILL: libc::c_int = -6;

pub const POLL_IN: libc::c_int = 1;
pub const POLL_OUT: libc::c_int = 2;
pub const POLL_MSG: libc::c_int = 3;
pub const POLL_ERR: libc::c_int = 4;
pub const POLL_PRI: libc::c_int = 5;
pub const POLL_HUP: libc::c_int = 6;

pub type SigHandler = unsafe extern "C" fn(libc::c_int);
pub type SigAction = unsafe extern "C" fn(libc::c_int, *mut siginfo_t, *mut libc::c_void);

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy)]
pub union sigval {
    pub sigval_int: libc::c_int,
    pub sigval_ptr: *mut libc::c_void,
}

impl Clone for sigval {
    fn clone(&self) -> Self {
        unsafe { mem::transmute_copy(self) }
    }
}

impl Default for sigval {
    fn default() -> Self {
        Self {
            sigval_ptr: ptr::null_mut(),
        }
    }
}

type T = libc::siginfo_t;

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Default)]
pub struct siginfo_t {
    pub si_signo: libc::c_int,
    pub si_errno: libc::c_int,
    pub si_code: libc::c_int,
    pub _pad: [libc::c_int; 29],
    _align: [u64; 0],
}

impl siginfo_t {
    unsafe fn variant_kill(&mut self) -> &mut siginfo_kill {
        &mut *(self as *mut siginfo_t as *mut siginfo_kill)
    }

    unsafe fn variant_sigqueue(&mut self) -> &mut siginfo_sigqueue {
        &mut *(self as *mut siginfo_t as *mut siginfo_sigqueue)
    }

    unsafe fn variant_timer(&mut self) -> &mut siginfo_timer {
        &mut *(self as *mut siginfo_t as *mut siginfo_timer)
    }

    unsafe fn variant_sigchld(&mut self) -> &mut sifields_sigchld {
        &mut (*(self as *mut siginfo_t as *mut siginfo_f))
            .sifields
            .sigchld
    }

    unsafe fn variant_poll(&mut self) -> &mut siginfo_poll {
        &mut *(self as *mut siginfo_t as *mut siginfo_poll)
    }

    pub fn from_raised(raised_info: RaisedSignalInfo) -> Self {
        let mut siginfo = Self::default();

        match raised_info {
            RaisedSignalInfo::Kill(i) => {
                let kill = unsafe { siginfo.variant_kill() };
                kill.si_code = SI_USER;
                kill.si_signo = i.signum;
                kill.si_pid = i.pid;
                kill.si_uid = i.uid;
            }
            RaisedSignalInfo::SigQueue(i) => {
                let sigqueue = unsafe { siginfo.variant_sigqueue() };
                sigqueue.si_code = SI_QUEUE;
                sigqueue.si_signo = i.signum;
                sigqueue.si_pid = i.pid;
                sigqueue.si_uid = i.uid;
                sigqueue.si_sigval = i.value;
            }
            RaisedSignalInfo::Timer(i) => {
                let timer = unsafe { siginfo.variant_timer() };
                timer.si_code = SI_TIMER;
                timer.si_signo = i.signum;
                timer.si_tid = i.timer_id;
                timer.si_overrun = i.overrun;
            }
            RaisedSignalInfo::MessageQueue(i) => {
                let queue = unsafe { siginfo.variant_sigqueue() };
                queue.si_code = SI_MESGQ;
                queue.si_signo = i.signum;
                queue.si_pid = i.pid;
                queue.si_uid = i.uid;
                queue.si_sigval = i.value;
            }
            RaisedSignalInfo::Child(i) => {
                siginfo.si_code = i.code.into();
                siginfo.si_signo = libc::SIGCHLD;
                let child = unsafe { siginfo.variant_sigchld() };
                child.si_pid = i.pid;
                child.si_uid = i.uid;
                child.si_status = i.status;
                // NOTE: stime, utime left unset here because the man page does not mention them
            }
            RaisedSignalInfo::Io(i) => {
                let poll = unsafe { siginfo.variant_poll() };
                poll.si_code = i.code.into();
                poll.si_signo = libc::SIGIO;
                poll.si_fd = i.fd;
                poll.si_band = i.band;
            }
            RaisedSignalInfo::TKill(i) => {
                let kill = unsafe { siginfo.variant_kill() };
                kill.si_code = SI_TKILL;
                kill.si_signo = i.signum;
                kill.si_pid = i.pid;
                kill.si_uid = i.uid;
            }
            RaisedSignalInfo::Aio(i) => {
                siginfo.si_code = SI_ASYNCIO;
                siginfo.si_signo = i.signum;
                let sigqueue = unsafe { siginfo.variant_sigqueue() };
                sigqueue.si_sigval = i.value;
            }
        }

        siginfo
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct siginfo_kill {
    pub si_signo: libc::c_int,
    pub si_errno: libc::c_int,
    pub si_code: libc::c_int,
    pub si_pid: libc::pid_t,
    pub si_uid: libc::uid_t,
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct siginfo_sigqueue {
    pub si_signo: libc::c_int,
    pub si_errno: libc::c_int,
    pub si_code: libc::c_int,
    pub si_pid: libc::pid_t,
    pub si_uid: libc::uid_t,
    pub si_sigval: sigval,
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct siginfo_timer {
    pub si_signo: libc::c_int,
    pub si_errno: libc::c_int,
    pub si_code: libc::c_int,
    pub si_tid: libc::pid_t,
    pub si_overrun: libc::c_int,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Clone, Copy)]
struct sifields_sigchld {
    pub si_pid: libc::pid_t,
    pub si_uid: libc::uid_t,
    pub si_status: libc::c_int,
    pub si_utime: libc::c_long,
    pub si_stime: libc::c_long,
}

// Internal, for casts to access union fields. Note that some variants
// of sifields start with a pointer, which makes the alignment of
// sifields vary on 32-bit and 64-bit architectures.
#[allow(non_camel_case_types)]
#[repr(C)]
struct siginfo_f {
    _siginfo_base: [libc::c_int; 3],
    sifields: sifields,
}

// Internal, for casts to access union fields
#[repr(C)]
union sifields {
    _align_pointer: *mut libc::c_void,
    sigchld: sifields_sigchld,
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct siginfo_poll {
    pub si_signo: libc::c_int,
    pub si_errno: libc::c_int,
    pub si_code: libc::c_int,
    pub si_band: libc::c_long,
    pub si_fd: libc::c_int,
}

#[derive(Clone, Copy, Debug)]
pub enum RaisedSignalInfo {
    Aio(SigAioInfo),
    Kill(SigKillInfo),
    SigQueue(SigQueueInfo),
    Timer(SigTimerInfo),
    MessageQueue(SigMqInfo),
    Child(SigChildInfo),
    Io(SigIoInfo),
    TKill(SigKillInfo),
}

impl RaisedSignalInfo {
    pub fn signum(&self) -> i32 {
        match self {
            RaisedSignalInfo::Aio(i) => i.signum,
            RaisedSignalInfo::Kill(i) => i.signum,
            RaisedSignalInfo::SigQueue(i) => i.signum,
            RaisedSignalInfo::Timer(i) => i.signum,
            RaisedSignalInfo::MessageQueue(i) => i.signum,
            RaisedSignalInfo::Child(_) => libc::SIGCHLD,
            RaisedSignalInfo::Io(_) => libc::SIGIO,
            RaisedSignalInfo::TKill(i) => i.signum,
        }
    }

    pub fn pid(&self) -> Option<libc::pid_t> {
        match self {
            RaisedSignalInfo::Aio(_) => None,
            RaisedSignalInfo::Kill(i) => Some(i.pid),
            RaisedSignalInfo::SigQueue(i) => Some(i.pid),
            RaisedSignalInfo::Timer(_) => None,
            RaisedSignalInfo::MessageQueue(i) => Some(i.pid),
            RaisedSignalInfo::Child(i) => Some(i.pid),
            RaisedSignalInfo::Io(_) => None,
            RaisedSignalInfo::TKill(i) => Some(i.pid),
        }
    }

    pub fn uid(&self) -> Option<libc::uid_t> {
        match self {
            RaisedSignalInfo::Aio(_) => None,
            RaisedSignalInfo::Kill(i) => Some(i.uid),
            RaisedSignalInfo::SigQueue(i) => Some(i.uid),
            RaisedSignalInfo::Timer(_) => None,
            RaisedSignalInfo::MessageQueue(i) => Some(i.uid),
            RaisedSignalInfo::Child(i) => Some(i.uid),
            RaisedSignalInfo::Io(_) => None,
            RaisedSignalInfo::TKill(i) => Some(i.uid),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SigKillInfo {
    pub signum: libc::c_int,
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
}

#[derive(Clone, Copy)]
pub struct SigQueueInfo {
    pub signum: libc::c_int,
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
    pub value: sigval,
}

impl Debug for SigQueueInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigQueueInfo")
            .field("signum", &self.signum)
            .field("pid", &self.pid)
            .field("uid", &self.uid)
            .field("value", &"<union>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SigTimerInfo {
    pub signum: libc::c_int,
    pub overrun: libc::c_int,
    // The man pages say this is an internal ID used by the kernel and that it is not the
    // same as the timer ID returned by `timer_create()`. We set it to the TimerId value.
    pub timer_id: libc::c_int,
}

#[derive(Clone, Copy)]
pub struct SigMqInfo {
    pub signum: libc::c_int,
    pub value: sigval,
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
}

impl Debug for SigMqInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigMqInfo")
            .field("signum", &self.signum)
            .field("value", &"<union>")
            .field("pid", &self.pid)
            .field("uid", &self.uid)
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SigChildInfo {
    pub code: SigChildCode, // mapped from libc::c_int
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
    pub status: libc::c_int,
}

#[derive(Clone, Copy)]
pub struct SigAioInfo {
    pub signum: libc::c_int,
    pub value: sigval,
}

impl Debug for SigAioInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigAioInfo")
            .field("signum", &self.signum)
            .field("value", &"<union>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SigChildCode {
    Exited,
    Killed,
    Dumped,
    Trapped,
    Stopped,
    Continued,
}

impl From<SigChildCode> for i32 {
    fn from(value: SigChildCode) -> Self {
        match value {
            SigChildCode::Exited => libc::CLD_EXITED,
            SigChildCode::Killed => libc::CLD_KILLED,
            SigChildCode::Dumped => libc::CLD_DUMPED,
            SigChildCode::Trapped => libc::CLD_TRAPPED,
            SigChildCode::Stopped => libc::CLD_STOPPED,
            SigChildCode::Continued => libc::CLD_CONTINUED,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SigIoInfo {
    pub code: SigIoCode,
    /// The `revents` filled in by `poll()`
    pub band: libc::c_long,
    pub fd: libc::c_int,
}

#[derive(Clone, Copy, Debug)]
pub enum SigIoCode {
    PollIn,
    PollOut,
    PollMsg,
    PollErr,
    PollPri,
    PollHup,
}

impl From<SigIoCode> for i32 {
    fn from(value: SigIoCode) -> Self {
        match value {
            SigIoCode::PollIn => POLL_IN,
            SigIoCode::PollOut => POLL_OUT,
            SigIoCode::PollMsg => POLL_MSG,
            SigIoCode::PollErr => POLL_ERR,
            SigIoCode::PollPri => POLL_PRI,
            SigIoCode::PollHup => POLL_HUP,
        }
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SignalSet: u32 {
        const SIGHUP    = 1 << 0;
        const SIGINT    = 1 << 1;
        const SIGQUIT   = 1 << 2;
        const SIGILL    = 1 << 3;
        const SIGTRAP   = 1 << 4;
        const SIGABRT   = 1 << 5;
        const SIGIOT    = 1 << 5;
        const SIGBUS    = 1 << 6;
        const SIGFPE    = 1 << 7;
        const SIGKILL   = 1 << 8;
        const SIGUSR1   = 1 << 9;
        const SIGSEGV   = 1 << 10;
        const SIGUSR2   = 1 << 11;
        const SIGPIPE   = 1 << 12;
        const SIGALRM   = 1 << 13;
        const SIGTERM   = 1 << 14;
        const SIGSTKFLT = 1 << 15;
        const SIGCHLD   = 1 << 16;
        const SIGCONT   = 1 << 17;
        const SIGSTOP   = 1 << 18;
        const SIGTSTP   = 1 << 19;
        const SIGTTIN   = 1 << 20;
        const SIGTTOU   = 1 << 21;
        const SIGURG    = 1 << 22;
        const SIGXCPU   = 1 << 23;
        const SIGXFSZ   = 1 << 24;
        const SIGVTALRM = 1 << 25;
        const SIGGPROF  = 1 << 26;
        const SIGWINCH  = 1 << 27;
        const SIGIO     = 1 << 28;
        const SIGPOLL   = 1 << 28;
        const SIGLOST   = 1 << 28;
        const SIGPWR    = 1 << 29;
        const SIGSYS    = 1 << 30;
        const SIGUNUSED = 1 << 30;
        const SIGRTMIN  = 1 << 31;
    }
}

impl SignalSet {
    /// Returns the value of the lowest-numbered signal present in the set.
    pub fn from_signum(signum: libc::c_int) -> Self {
        Self::from_bits((1 << (signum - 1)) as u32).unwrap()
    }

    pub fn from_sigset(set: libc::sigset_t) -> Self {
        let mut raw_set = 0u32;
        for i in 0u32..=31u32 {
            if unsafe { libc::sigismember(ptr::addr_of!(set), (i + 1) as i32) } != 0 {
                raw_set |= 1 << i;
            }
        }

        Self::from_bits(raw_set).unwrap()
    }

    pub fn to_sigset(&self) -> libc::sigset_t {
        let mut set = MaybeUninit::<libc::sigset_t>::uninit();
        let mut set = unsafe {
            libc::sigemptyset(set.as_mut_ptr());
            set.assume_init()
        };

        for i in 0u64..=31u64 {
            if self.bits() & (1 << i) > 0 {
                unsafe {
                    libc::sigaddset(ptr::addr_of_mut!(set), (i + 1) as i32);
                }
            }
        }

        set
    }

    /// Returns the value of the lowest-numbered signal present in the set.
    pub fn lowest_signal_value(&self) -> libc::c_int {
        (self.bits().count_zeros() + 1) as libc::c_int
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SigDisposition {
    #[default]
    Default,
    Ignore,
    Handler(SigHandler),
    Action(SigAction),
}

#[derive(Clone, Debug)]
pub struct ThreadSigInfo {
    /// Signals that have been specifically raised for the thread via `pthread_kill` but that cannot
    /// be immediately handled as they are blocked.
    pub raised: RaisedSignalSet,
    /// Signals that have been masked for the given thread.
    ///
    /// Note that when `SigCallback::Ignore` is set, any blocked signals will be discarded.
    pub blocked: SignalSet,
    /// Indicates that the given thread is currently waiting on a blocked signal to become pending
    pub sigwait_set: SignalSet,
    pub sigsuspend: bool,
}

impl ThreadSigInfo {
    /// Inherits the set of blocked signals from another thread.
    pub fn inherit(sigmask: SignalSet) -> Self {
        Self {
            raised: array::from_fn(|_| None), // Raised signals are not inherited
            blocked: sigmask,                 // Blocked signals are inherited
            sigwait_set: SignalSet::empty(),
            sigsuspend: false,
        }
    }

    pub fn new(sigmask: Option<SignalSet>) -> Self {
        let blocked = match sigmask {
            Some(s) => s,
            None => SignalSet::empty(),
        };

        Self {
            raised: array::from_fn(|_| None), // TODO: check all these
            blocked,
            sigwait_set: SignalSet::empty(),
            sigsuspend: false,
        }
    }
}

pub struct SignalSetHandlerEvent {
    signum: libc::c_int,
    disposition: Option<SigDisposition>,
}

impl SignalSetHandlerEvent {
    pub fn new(signum: libc::c_int, disposition: Option<SigDisposition>) -> Self {
        Self {
            signum,
            disposition,
        }
    }
}

impl Event for SignalSetHandlerEvent {
    type Success = SigDisposition;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let process_id = state.local.process_id;
        let handler = &mut state
            .global
            .processes
            .get_mut(&process_id)
            .unwrap()
            .signal_handlers[self.signum as usize];

        if let Some(mut tmp_handler) = self.disposition.clone() {
            mem::swap(&mut tmp_handler, handler);
            Outcome::Success(tmp_handler)
        } else {
            Outcome::Success(handler.clone())
        }
    }
}

#[derive(Clone, Debug)]
pub enum SignalTarget {
    Pid(WorkerId),
    CallingProcessGroup,
    AllPermissive,
    Pgid(ProcessGroupId),
    Thread(libc::pthread_t),
    Tid(WorkerId, libc::pid_t),
}

pub enum SignalSendState {
    Prepare,
    SendSignals(Vec<SignalDestination>),
}

pub struct SignalSendEvent {
    target: SignalTarget,
    signum: libc::c_int,
    value: Option<sigval>,
    state: SignalSendState,
}

impl SignalSendEvent {
    pub fn new(target: SignalTarget, signum: libc::c_int, value: Option<sigval>) -> Self {
        Self {
            target,
            signum,
            value,
            state: SignalSendState::Prepare,
        }
    }
}

impl Event for SignalSendEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            SignalSendState::Prepare => {
                let mut destinations = Vec::new();

                match self.target {
                    SignalTarget::Pid(worker_id) => {
                        if state.global.ids.get(&worker_id).is_none() {
                            return Outcome::Error(Errno::ESRCH);
                        }

                        destinations.push(SignalDestination::Process(worker_id));
                    }
                    SignalTarget::CallingProcessGroup => {
                        let process_id = state.local.process_id;
                        let pgid = state.global.processes.get(&process_id).unwrap().pgid;

                        let processes: Vec<_> = state
                            .global
                            .process_groups
                            .get(&pgid)
                            .unwrap()
                            .iter()
                            .collect();
                        for process_id in processes {
                            let pid = state.global.processes.get(&process_id).unwrap().pid;
                            destinations.push(SignalDestination::Process(pid));
                        }
                    }
                    SignalTarget::AllPermissive => {
                        let processes: Vec<_> = state.global.processes.keys().collect();
                        for process_id in processes {
                            let pid = state.global.processes.get(&process_id).unwrap().pid;
                            destinations.push(SignalDestination::Process(pid));
                        }
                    }
                    SignalTarget::Pgid(pgid) => {
                        let processes: Vec<_> = state
                            .global
                            .process_groups
                            .get(&pgid)
                            .unwrap()
                            .iter()
                            .collect();
                        for process_id in processes {
                            let pid = state.global.processes.get(&process_id).unwrap().pid;
                            destinations.push(SignalDestination::Process(pid));
                        }
                    }
                    SignalTarget::Tid(tid, _pid) => {
                        let Some(_worker_info) = state.global.ids.get(&tid) else {
                            return Outcome::Error(Errno::ESRCH);
                        };

                        // TODO: check to see if `tid` exists within `pid`

                        destinations.push(SignalDestination::Thread(tid));
                    }
                    SignalTarget::Thread(pthread) => {
                        let Some(t) = state.local.pthreads.get(&pthread) else {
                            return Outcome::Error(Errno::EINVAL);
                        };

                        let tid = state.local.tids.get(&t.id).unwrap();

                        let Some(_worker_info) = state.global.ids.get(&tid) else {
                            return Outcome::Error(Errno::ESRCH);
                        };

                        // TODO: check to see if `tid` exists within `pid`

                        destinations.push(SignalDestination::Thread(*tid));
                    }
                }

                Outcome::Continue
            }
            SignalSendState::SendSignals(destinations) => {
                let Some(destination) = destinations.pop() else {
                    return Outcome::Success(());
                };

                let process_id = state.local.process_id;
                let pid = state.global.processes.get(&process_id).unwrap().pid;

                let raised_info = match self.value {
                    Some(value) => RaisedSignalInfo::SigQueue(SigQueueInfo {
                        signum: self.signum,
                        pid: pid.as_id(),
                        uid: unsafe { libc::getuid() },
                        value,
                    }),
                    None => RaisedSignalInfo::Kill(SigKillInfo {
                        signum: self.signum,
                        pid: pid.as_id(),
                        uid: unsafe { libc::getuid() },
                    }),
                };

                Outcome::SendSignal(destination, raised_info)
            }
        }
    }
}

pub enum SignalWaitState {
    Start,
    Finish,
}

pub struct SignalWaitEvent {
    wait_set: SignalSet,
    timeout: Option<Duration>,
    state: SignalWaitState,
}

impl SignalWaitEvent {
    pub fn new(wait_set: SignalSet, timeout: Option<Duration>) -> Self {
        Self {
            wait_set,
            timeout,
            state: SignalWaitState::Start,
        }
    }
}

impl Event for SignalWaitEvent {
    type Success = RaisedSignalInfo;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            SignalWaitState::Start => {
                let process_id = state.local.process_id;
                let worker_id = WorkerInfo::current(process_id);

                // Are there any blocked signals for this thread?
                let thread_signals = state
                    .local
                    .signals
                    .get_mut(&thread::current().id())
                    .unwrap();
                let blocked_signals = thread_signals.blocked;
                let raised_signals = &mut thread_signals.raised;

                let interest_signals = self.wait_set.intersection(blocked_signals);

                let mut ready = None;

                for signal in interest_signals.iter() {
                    if let Some(r) = raised_signals[signal.lowest_signal_value() as usize].take() {
                        ready = Some(r);
                        break;
                    }
                }

                if let Some(raised_info) = ready {
                    // yes--immediately use
                    return Outcome::Success(raised_info);
                }
                // No--check blocked signals for the process

                state
                    .local
                    .signals
                    .get_mut(&worker_id.thread_id)
                    .unwrap()
                    .sigwait_set = self.wait_set;

                self.state = SignalWaitState::Finish;
                Outcome::Yield(self.timeout)
            }
            SignalWaitState::Finish => {
                let siginfo = state
                    .local
                    .signals
                    .get_mut(&thread::current().id())
                    .unwrap();
                siginfo.sigwait_set = SignalSet::empty();

                for signal in self.wait_set {
                    if let Some(raised_info) =
                        siginfo.raised[signal.lowest_signal_value() as usize].take()
                    {
                        return Outcome::Success(raised_info);
                    }
                }

                Outcome::Error(Errno::ETIMEDOUT)
            }
        }
    }
}

pub struct SignalGetPendingEvent;

impl Event for SignalGetPendingEvent {
    type Success = SignalSet;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let mut set = SignalSet::empty();

        for (idx, raised) in state
            .local
            .signals
            .get(&thread::current().id())
            .unwrap()
            .raised
            .iter()
            .enumerate()
        {
            if raised.is_some() {
                set |= SignalSet::from_bits_truncate(idx as u32);
            }
        }

        Outcome::Success(set)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SigmaskOp {
    Block,
    Unblock,
    Setmask,
}

pub struct SignalSetSigmaskEvent {
    op: SigmaskOp,
    mask: Option<SignalSet>,
    old: Option<SignalSet>,
}

impl SignalSetSigmaskEvent {
    pub fn new(op: SigmaskOp, mask: Option<SignalSet>) -> Self {
        Self {
            op,
            mask,
            old: None,
        }
    }
}

impl Event for SignalSetSigmaskEvent {
    type Success = SignalSet;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let old = *self.old.get_or_insert(
            state
                .local
                .signals
                .get(&thread::current().id())
                .unwrap()
                .blocked,
        );
        let Some(mask) = self.mask else {
            return Outcome::Success(old);
        };

        let (new, unblocked) = match self.op {
            SigmaskOp::Block => (old | mask, SignalSet::empty()),
            SigmaskOp::Setmask => (mask, old & !mask),
            SigmaskOp::Unblock => (old & !mask, old & mask),
        };

        let pid = *state.local.tids.get(&thread::current().id()).unwrap();

        let siginfo = state
            .local
            .signals
            .get_mut(&thread::current().id())
            .unwrap();
        siginfo.blocked |= new; // Add newly blocked signals
        for signal in unblocked {
            siginfo.blocked -= signal; // Incrementally remove old signals, handling each if necessary
            if let Some(raised_info) = siginfo.raised[signal.lowest_signal_value() as usize].take()
            {
                // This entire function runs for as many times as is needed to handle all signals
                return Outcome::SendSignal(SignalDestination::Thread(pid), raised_info);
            }
        }

        Outcome::Success(old)
    }
}

pub enum SignalSuspendState {
    Start,
    Finish(SignalSet),
}

pub struct SignalSuspendEvent {
    mask: SignalSet,
    state: SignalSuspendState,
}

impl SignalSuspendEvent {
    pub fn new(mask: SignalSet) -> Self {
        Self {
            mask,
            state: SignalSuspendState::Start,
        }
    }
}

impl Event for SignalSuspendEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let thread_id = thread::current().id();

        match self.state {
            SignalSuspendState::Start => {
                let old = state.local.signals.get(&thread_id).unwrap().blocked;
                self.state = SignalSuspendState::Finish(old);

                let unblocked = old & !self.mask;

                let pid = *state.local.tids.get(&thread_id).unwrap();

                let siginfo = state.local.signals.get_mut(&thread_id).unwrap();
                for signal in unblocked {
                    if let Some(raised_info) =
                        siginfo.raised[signal.lowest_signal_value() as usize].take()
                    {
                        // This entire function runs for as many times as is needed to handle all signals
                        return Outcome::SendSignal(SignalDestination::Thread(pid), raised_info);
                    }
                }

                let signal_info = state.local.signals.get_mut(&thread_id).unwrap();
                signal_info.blocked = self.mask;
                signal_info.sigsuspend = true;
                Outcome::Yield(None)
            }
            SignalSuspendState::Finish(old) => {
                let signal_info = state.local.signals.get_mut(&thread_id).unwrap();
                signal_info.blocked = old;
                signal_info.sigsuspend = false;

                Outcome::Error(Errno::EINTR)
            }
        }
    }
}
