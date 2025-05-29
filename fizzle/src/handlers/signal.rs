use std::cell::RefCell;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::rc::Rc;
use std::time::Duration;
use std::{cmp, mem, ptr, slice, thread};

use bitflags::bitflags;

use crate::errno::Errno;
use crate::scheduler::{
    fizzle_alloc, Event, FizzleSingleton, HandleProcessSignalTask, HandleThreadSignalTask, Outcome,
    YieldUntil,
};
use crate::state::{FizzleState, SignalDestination};
use crate::task::{Task, TaskResult};
use crate::GlobalRc;

use super::descriptor::{Descriptor, DescriptorInfo, FdResource, ReadData};
use super::id::Worker;
use super::polled::PolledInfo;
use super::process::{Pgid, Pid};
use super::thread::Tid;

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
        &mut *(ptr::from_mut(self).cast::<siginfo_kill>())
    }

    unsafe fn variant_sigqueue(&mut self) -> &mut siginfo_sigqueue {
        &mut *(ptr::from_mut(self).cast::<siginfo_sigqueue>())
    }

    unsafe fn variant_timer(&mut self) -> &mut siginfo_timer {
        &mut *(ptr::from_mut(self).cast::<siginfo_timer>())
    }

    unsafe fn variant_sigchld(&mut self) -> &mut sifields_sigchld {
        &mut (*(ptr::from_mut(self).cast::<siginfo_f>()))
            .sifields
            .sigchld
    }

    unsafe fn variant_poll(&mut self) -> &mut siginfo_poll {
        &mut *(ptr::from_mut(self).cast::<siginfo_poll>())
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

    pub fn as_signalfd_info(&self) -> libc::signalfd_siginfo {
        let info = siginfo_t::from_raised(*self);

        let (ssi_fd, ssi_band) = match self {
            RaisedSignalInfo::Io(io) => (io.fd, io.band as u32),
            _ => (-1, 0),
        };

        let (ssi_tid, ssi_overrun) = match self {
            RaisedSignalInfo::Timer(t) => (t.timer_id as u32, t.overrun as u32),
            _ => (0, 0),
        };

        let ssi_status = match self {
            RaisedSignalInfo::Child(c) => c.status,
            _ => 0,
        };

        let (ssi_int, ssi_ptr) = match self {
            RaisedSignalInfo::SigQueue(s) => unsafe {
                (s.value.sigval_int, s.value.sigval_ptr.addr() as u64)
            },
            _ => (0, 0),
        };

        let mut si: libc::signalfd_siginfo = unsafe { MaybeUninit::zeroed().assume_init() };

        si.ssi_signo = info.si_signo as u32;
        si.ssi_errno = 0;
        si.ssi_code = info.si_code;
        si.ssi_pid = self.pid().unwrap_or(0) as u32;
        si.ssi_uid = self.uid().unwrap_or(0) as u32;
        si.ssi_fd = ssi_fd;
        si.ssi_tid = ssi_tid;
        si.ssi_band = ssi_band;
        si.ssi_overrun = ssi_overrun;
        si.ssi_status = ssi_status;
        si.ssi_int = ssi_int;
        si.ssi_ptr = ssi_ptr;

        si
    }
}

unsafe impl Send for RaisedSignalInfo {}

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
    pub pending: RaisedSignalSet,
    /// Signals that have been masked for the given thread.
    ///
    /// Note that when `SigCallback::Ignore` is set, any blocked signals will be discarded.
    pub masked: SignalSet,
    /// Indicates that the given thread is currently waiting on a blocked signal to become pending
    pub sigwait_set: SignalSet,
    pub sigsuspend: bool,
    pub interrupted: bool,
}

pub struct SignalfdInfo {
    pub mask: SignalSet,
    pub polled: GlobalRc<PolledInfo>,
    /// A counter that indicates the number of unique signals currently raised for the given mask.
    /// Used to bookkeep the `polled` ready value.
    pub num_raised: usize,
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct SignalfdFlags: i32 {
        const NONBLOCK = libc::SFD_NONBLOCK;
        const CLOSE_ON_EXEC = libc::SFD_CLOEXEC;
    }
}

pub struct SignalfdCreateEvent {
    fd: Option<Descriptor>,
    mask: SignalSet,
    flags: SignalfdFlags,
}

impl SignalfdCreateEvent {
    pub fn new(fd: Option<Descriptor>, mask: SignalSet, flags: SignalfdFlags) -> Self {
        Self { fd, mask, flags }
    }
}

impl Event for SignalfdCreateEvent {
    type Success = Descriptor;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.fd {
            Some(fd) => {
                // Change the signal mask for the current descriptor
                let Some(fd_info) = state.local.fds.get_mut(&fd) else {
                    return Outcome::Error(Errno::EBADFD);
                };

                let FdResource::Signalfd(signalfd) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::EINVAL);
                };

                let mut signalfd_mut = signalfd.borrow_mut();
                signalfd_mut.mask = self.mask;

                let mut num_raised = 0;
                for signal in self.mask {
                    if state
                        .local
                        .signals
                        .get(&thread::current().id())
                        .unwrap()
                        .pending[signal.lowest_signal_value() as usize - 1]
                        .is_some()
                    {
                        num_raised += 1;
                    }

                    if state.local.pending_signals[signal.lowest_signal_value() as usize - 1]
                        .is_some()
                    {
                        num_raised += 1;
                    }
                }

                let old_num_raised = signalfd_mut.num_raised;
                if old_num_raised == 0 && num_raised > 0 {
                    state.raise_polled(&signalfd_mut.polled);
                } else if num_raised == 0 && old_num_raised > 0 {
                    state.raise_polled(&signalfd_mut.polled);
                }

                signalfd_mut.num_raised = num_raised;

                Outcome::Success(fd)
            }
            None => {
                // Create a new descriptor with the given signal mask
                let fd = Descriptor::from_raw_fd(crate::create_descriptor());

                let mut num_raised = 0;
                for signal in self.mask {
                    if state
                        .local
                        .signals
                        .get(&thread::current().id())
                        .unwrap()
                        .pending[signal.lowest_signal_value() as usize - 1]
                        .is_some()
                    {
                        num_raised += 1;
                    }

                    if state.local.pending_signals[signal.lowest_signal_value() as usize - 1]
                        .is_some()
                    {
                        num_raised += 1;
                    }
                }

                let polled = Rc::new_in(
                    RefCell::new(PolledInfo {
                        pollers: Vec::new_in(fizzle_alloc()),
                        event_raised: false,
                    }),
                    fizzle_alloc(),
                );

                if num_raised > 0 {
                    state.raise_polled(&polled);
                }

                let signalfd = Rc::new_in(
                    RefCell::new(SignalfdInfo {
                        mask: self.mask,
                        num_raised,
                        polled,
                    }),
                    fizzle_alloc(),
                );

                let prev = state
                    .local
                    .process_info
                    .borrow_mut()
                    .signal_fds
                    .insert(thread::current().id(), signalfd.clone());
                if prev.is_some() {
                    panic!(
                        "Fizzle internal error: only one signalfd per thread currently supported"
                    )
                    // signlfds aren't implemented correctly in Fizzle for thread-specific signals.
                    // I believe a signalfd is meant to capture thread-specific signals in the context it is currently polling or reading,
                    // rather than the context in which it is created. The man pages are unclear on this though.
                }

                state.local.fds.insert(
                    fd,
                    DescriptorInfo {
                        close_on_exec: self.flags.contains(SignalfdFlags::CLOSE_ON_EXEC),
                        nonblocking: self.flags.contains(SignalfdFlags::NONBLOCK),
                        is_passthrough: false,
                        is_random: false,
                        resource: FdResource::Signalfd(signalfd),
                    },
                );

                Outcome::Success(fd)
            }
        }
    }
}

pub struct SignalfdReadEvent<'a> {
    info: GlobalRc<SignalfdInfo>,
    data: ReadData<'a>,
}

impl<'a> SignalfdReadEvent<'a> {
    pub fn new(info: GlobalRc<SignalfdInfo>, data: ReadData<'a>) -> Self {
        Self { info, data }
    }
}

impl<'a> Event for SignalfdReadEvent<'a> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if matches!(&self.data, ReadData::Socket(_, _)) {
            return Outcome::Error(Errno::ENOTSOCK);
        }

        if matches!(&self.data, ReadData::File(_)) {
            return Outcome::Error(Errno::ESPIPE);
        }

        let mut iov_idx = 0;
        let mut iov_offset = 0;
        let mut total_read = 0;

        if self.data.len() == 0 {
            return Outcome::Error(Errno::EINVAL);
        }

        // First, check thread_local pending signals
        let signalfd_ref = self.info.borrow();
        let proc_siginfo = state.local.process_info.clone();

        for signal in signalfd_ref.mask {
            if let Some(raised) = state
                .local
                .signals
                .get_mut(&thread::current().id())
                .unwrap()
                .pending[signal.lowest_signal_value() as usize - 1]
                .take()
            {
                // First we need to handle the decrement of the signalfd for this thread
                if let Some(signalfd) = proc_siginfo
                    .borrow()
                    .signal_fds
                    .get(&thread::current().id())
                {
                    let mut signalfd_mut = signalfd.borrow_mut();
                    if signalfd_mut
                        .mask
                        .contains(SignalSet::from_signum(raised.signum()))
                    {
                        if signalfd_mut.num_raised == 1 {
                            state.lower_polled(&signalfd_mut.polled);
                        }
                        signalfd_mut.num_raised -= 1;
                    }
                }

                let signalfd_info = raised.as_signalfd_info();
                let mut signalfd_info_bytes = unsafe {
                    slice::from_raw_parts(
                        (&raw const signalfd_info).cast::<u8>(),
                        mem::size_of_val(&signalfd_info),
                    )
                };

                loop {
                    let ioslice = match &mut self.data {
                        ReadData::BasicSlice(s) => &mut s[iov_offset..],
                        ReadData::Iovec(iov) => &mut iov[iov_idx][iov_offset..],
                        _ => unreachable!(),
                    };
                    let iov_rem = ioslice.len();
                    let copy_len = cmp::min(iov_rem, signalfd_info_bytes.len());
                    ioslice[..copy_len].copy_from_slice(&signalfd_info_bytes[..copy_len]);
                    total_read += copy_len;

                    if copy_len < iov_rem {
                        iov_offset += copy_len;
                        break;
                    } else {
                        signalfd_info_bytes = &signalfd_info_bytes[copy_len..];
                        iov_idx += 1;
                        iov_offset = 0;
                        match &self.data {
                            ReadData::BasicSlice(_) => return Outcome::Success(total_read),
                            ReadData::Iovec(iov) if iov_offset == iov.len() => {
                                return Outcome::Success(total_read)
                            }
                            _ => (),
                        }
                    }
                }
            }

            if let Some(raised) =
                state.local.pending_signals[signal.lowest_signal_value() as usize - 1].take()
            {
                // The signal is removed, so we need to decrement applicable `signalfd`s.
                let process_info = state.local.process_info.clone();
                for signalfd_info in process_info.borrow().signal_fds.values() {
                    let mut signalfd_mut = signalfd_info.borrow_mut();
                    if signalfd_mut
                        .mask
                        .contains(SignalSet::from_signum(raised.signum()))
                    {
                        if signalfd_mut.num_raised == 1 {
                            state.lower_polled(&signalfd_mut.polled);
                        }
                        signalfd_mut.num_raised -= 1;
                    }
                }

                let signalfd_info = raised.as_signalfd_info();
                let mut signalfd_info_bytes = unsafe {
                    slice::from_raw_parts(
                        (&raw const signalfd_info).cast::<u8>(),
                        mem::size_of_val(&signalfd_info),
                    )
                };

                loop {
                    let ioslice = match &mut self.data {
                        ReadData::BasicSlice(s) => &mut s[iov_offset..],
                        ReadData::Iovec(iov) => &mut iov[iov_idx][iov_offset..],
                        _ => unreachable!(),
                    };
                    let iov_rem = ioslice.len();
                    let copy_len = cmp::min(iov_rem, signalfd_info_bytes.len());
                    ioslice[..copy_len].copy_from_slice(&signalfd_info_bytes[..copy_len]);
                    total_read += copy_len;

                    if copy_len < iov_rem {
                        iov_offset += copy_len;
                        break;
                    } else {
                        signalfd_info_bytes = &signalfd_info_bytes[copy_len..];
                        iov_idx += 1;
                        iov_offset = 0;
                        match &self.data {
                            ReadData::BasicSlice(_) => return Outcome::Success(total_read),
                            ReadData::Iovec(iov) if iov_offset == iov.len() => {
                                return Outcome::Success(total_read)
                            }
                            _ => (),
                        }
                    }
                }
            }
        }

        Outcome::Success(total_read)
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
        let mut proc_info_borrow = state.local.process_info.borrow_mut();
        let handler = &mut proc_info_borrow.signal_handlers[self.signum as usize - 1];

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
    Pid(Pid),
    CallingProcessGroup,
    AllPermissive,
    Pgid(Pgid),
    Thread(libc::pthread_t),
    Tid(Tid, Pid),
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
                    SignalTarget::Pid(pid) => {
                        if !state.global.pids.contains_key(&pid) {
                            return Outcome::Error(Errno::ESRCH);
                        };

                        // TODO: change SignalDestination to be passed an Rc<ProcessInfo>?
                        destinations.push(SignalDestination::Process(pid));
                    }
                    SignalTarget::CallingProcessGroup => {
                        let pgid = state.local.process_info.borrow().pgid;

                        let processes: Vec<_> = state
                            .global
                            .process_groups
                            .get(&pgid)
                            .unwrap()
                            .iter()
                            .collect();
                        for dst_pid in processes {
                            // TODO: check to see if process exists?
                            destinations.push(SignalDestination::Process(*dst_pid));
                        }
                    }
                    SignalTarget::AllPermissive => {
                        // TODO: send signal to all children? All children + in group? All but startup processes?
                        let pids: Vec<_> = state.global.pids.keys().collect();
                        for pid in pids {
                            //TODO: check to see if processes exist?
                            destinations.push(SignalDestination::Process(*pid));
                        }
                    }
                    SignalTarget::Pgid(pgid) => {
                        let Some(pids) = state.global.process_groups.get(&pgid) else {
                            return Outcome::Error(Errno::ESRCH); // TODO: esrch?
                        };

                        for pid in pids.iter() {
                            destinations.push(SignalDestination::Process(*pid));
                        }
                    }
                    SignalTarget::Tid(tid, _pid) => {
                        let Some(thread_id) = state.local.tid_threads.get(&tid) else {
                            return Outcome::Error(Errno::ESRCH);
                        };

                        let pid = state.local.process_info.borrow().pid;

                        // TODO: check to see if `tid` exists within `pid`

                        destinations.push(SignalDestination::Thread(pid, *thread_id));
                    }
                    SignalTarget::Thread(pthread) => {
                        let Some(t) = state.local.pthreads.get(&pthread) else {
                            return Outcome::Error(Errno::EINVAL);
                        };

                        let pid = state.local.process_info.borrow().pid;
                        let thread_id = t.id;

                        // TODO: check to see if `tid` exists within `pid`

                        destinations.push(SignalDestination::Thread(pid, thread_id));
                    }
                }

                self.state = SignalSendState::SendSignals(destinations);
                Outcome::Yield(YieldUntil::Immediate)
            }
            SignalSendState::SendSignals(destinations) => {
                let Some(destination) = destinations.pop() else {
                    return Outcome::Success(());
                };

                let pid = state.local.process_info.borrow().pid;

                let raised = match self.value {
                    Some(value) => RaisedSignalInfo::SigQueue(SigQueueInfo {
                        signum: self.signum,
                        pid: pid.as_raw(),
                        uid: unsafe { libc::getuid() },
                        value,
                    }),
                    None => RaisedSignalInfo::Kill(SigKillInfo {
                        signum: self.signum,
                        pid: pid.as_raw(),
                        uid: unsafe { libc::getuid() },
                    }),
                };

                Outcome::RunTask(
                    Task::SendSignal(SendSignalTask {
                        destination,
                        raised,
                    }),
                    YieldUntil::Reschedule(Duration::ZERO),
                )
            }
        }
    }
}

pub struct SendSignalTask {
    destination: SignalDestination,
    raised: RaisedSignalInfo,
}

impl SendSignalTask {
    pub fn execute(self, ctx: &mut FizzleSingleton) -> TaskResult {
        let raised = self.raised;
        match self.destination {
            SignalDestination::Process(pid) => {
                log::debug!("Sending signal to process {:?}", pid);
                HandleProcessSignalTask { raised, pid }.execute(ctx)
            }
            SignalDestination::Thread(pid, thread_id) => {
                let worker = Worker { pid, thread_id };
                log::debug!("Sending signal to worker {:?}", worker);
                HandleThreadSignalTask {
                    raised,
                    dst: worker,
                }
                .execute(ctx)
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
                let worker_id = state.current_worker();

                // Are there any blocked signals for this thread?
                let thread_signals = state
                    .local
                    .signals
                    .get_mut(&thread::current().id())
                    .unwrap();
                let thread_raised_signals = &mut thread_signals.pending;
                let interest_signals = self.wait_set;

                let mut ready = None;

                for signal in interest_signals.iter() {
                    if let Some(r) =
                        thread_raised_signals[signal.lowest_signal_value() as usize - 1].take()
                    {
                        ready = Some(r);

                        // We need to update the signalfd for this process, if applicable
                        let proc_info = state.local.process_info.clone();
                        if let Some(signalfd) =
                            proc_info.borrow().signal_fds.get(&thread::current().id())
                        {
                            let mut signalfd_mut = signalfd.borrow_mut();
                            if signalfd_mut.mask.contains(signal) {
                                if signalfd_mut.num_raised == 1 {
                                    state.lower_polled(&signalfd_mut.polled);
                                }
                                signalfd_mut.num_raised -= 1;
                            }
                        }

                        break;
                    }
                }

                let proc_raised_signals = &mut state.local.pending_signals;
                if ready.is_none() {
                    for signal in interest_signals.iter() {
                        if let Some(r) =
                            proc_raised_signals[signal.lowest_signal_value() as usize - 1].take()
                        {
                            ready = Some(r);

                            // We need to update the signalfd for this process, if applicable
                            let proc_info = state.local.process_info.clone();
                            for signalfd in proc_info.borrow().signal_fds.values() {
                                let mut signalfd_mut = signalfd.borrow_mut();
                                if signalfd_mut.mask.contains(signal) {
                                    if signalfd_mut.num_raised == 1 {
                                        state.lower_polled(&signalfd_mut.polled);
                                    }
                                    signalfd_mut.num_raised -= 1;
                                }
                            }

                            break;
                        }
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
                Outcome::Yield(match self.timeout {
                    Some(timeout) => YieldUntil::Reschedule(timeout),
                    None => YieldUntil::None,
                })
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
                        siginfo.pending[signal.lowest_signal_value() as usize - 1].take()
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
            .pending
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
        let siginfo = state
            .local
            .signals
            .get_mut(&thread::current().id())
            .unwrap();

        let old = *self.old.get_or_insert(siginfo.masked);
        let Some(mask) = self.mask else {
            return Outcome::Success(old);
        };

        let (new, unblocked) = match self.op {
            SigmaskOp::Block => (old | mask, SignalSet::empty()),
            SigmaskOp::Setmask => (mask, old & !mask),
            SigmaskOp::Unblock => (old & !mask, old & mask),
        };

        siginfo.masked |= new; // Add newly blocked signals
        let proc_info = state.local.process_info.clone();

        for signal in unblocked {
            // Incrementally remove old signals, handling each if necessary

            let local = &mut state.local;
            let proc_pending = &mut local.pending_signals;
            let siginfo = local.signals.get_mut(&thread::current().id()).unwrap();

            if let Some(raised_info) =
                siginfo.pending[signal.lowest_signal_value() as usize - 1].take()
            {
                // We need to update the signalfd for this process, if applicable
                if let Some(signalfd) = proc_info.borrow().signal_fds.get(&thread::current().id()) {
                    let mut signalfd_mut = signalfd.borrow_mut();
                    if signalfd_mut.mask.contains(signal) {
                        if signalfd_mut.num_raised == 1 {
                            state.lower_polled(&signalfd_mut.polled);
                        }
                        signalfd_mut.num_raised -= 1;
                    }
                }

                // This entire function runs for as many times as is needed to handle all signals
                return Outcome::RunTask(
                    Task::HandleThreadSignal(HandleThreadSignalTask {
                        raised: raised_info,
                        dst: state.current_worker(),
                    }),
                    YieldUntil::Reschedule(Duration::ZERO),
                );
            }

            siginfo.masked -= signal;

            if let Some(raised_info) =
                proc_pending[signal.lowest_signal_value() as usize - 1].take()
            {
                // We need to update the signalfd for this process, if applicable
                for signalfd in proc_info.borrow().signal_fds.values() {
                    let mut signalfd_mut = signalfd.borrow_mut();
                    if signalfd_mut.mask.contains(signal) {
                        if signalfd_mut.num_raised == 1 {
                            state.lower_polled(&signalfd_mut.polled);
                        }
                        signalfd_mut.num_raised -= 1;
                    }
                }

                // This entire function runs for as many times as is needed to handle all signals
                return Outcome::RunTask(
                    Task::HandleThreadSignal(HandleThreadSignalTask {
                        raised: raised_info,
                        dst: state.current_worker(),
                    }),
                    YieldUntil::Reschedule(Duration::ZERO),
                );
            }
        }

        Outcome::Success(old)
    }
}

pub enum SignalSuspendState {
    Start,
    HandlePending(SignalSet),
    Finish(SignalSet),
}

pub struct SignalSuspendEvent {
    mask: SignalSet,
    state: SignalSuspendState,
    received_signal: bool,
}

impl SignalSuspendEvent {
    pub fn new(mask: SignalSet) -> Self {
        Self {
            mask,
            state: SignalSuspendState::Start,
            received_signal: false,
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
                let old = state.local.signals.get(&thread_id).unwrap().masked;

                state.local.signals.get_mut(&thread_id).unwrap().masked = self.mask;
                self.state = SignalSuspendState::HandlePending(old);
                Outcome::Yield(YieldUntil::Immediate)
            }
            SignalSuspendState::HandlePending(old_mask) => {
                let unblocked = old_mask & !self.mask;

                let siginfo = state.local.signals.get_mut(&thread_id).unwrap();
                for signal in unblocked {
                    if let Some(raised_info) =
                        siginfo.pending[signal.lowest_signal_value() as usize - 1].take()
                    {
                        self.received_signal = true;

                        // This entire function runs for as many times as is needed to handle all signals
                        return Outcome::RunTask(
                            Task::HandleThreadSignal(HandleThreadSignalTask {
                                raised: raised_info,
                                dst: state.current_worker(),
                            }),
                            YieldUntil::Reschedule(Duration::ZERO),
                        );
                    }
                }

                if !self.received_signal {
                    self.state = SignalSuspendState::Finish(old_mask);
                    let signal_info = state.local.signals.get_mut(&thread_id).unwrap();
                    signal_info.sigsuspend = true;
                    Outcome::Yield(YieldUntil::None)
                } else {
                    self.state = SignalSuspendState::Finish(old_mask);
                    Outcome::Yield(YieldUntil::Immediate)
                }
            }
            SignalSuspendState::Finish(old_mask) => {
                let signal_info = state.local.signals.get_mut(&thread_id).unwrap();
                signal_info.masked = old_mask;
                signal_info.sigsuspend = false;

                Outcome::Error(Errno::EINTR)
            }
        }
    }
}
