use std::{mem, ptr, thread};
use std::mem::MaybeUninit;

use bitflags::bitflags;

use crate::arena::Rc;
use crate::handlers::polled::PolledId;
use crate::state::FizzleSingleton;

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

impl SignalSet {
    /// Returns the value of the lowest-numbered signal present in the set.
    pub fn from_signum(signum: libc::c_int) -> Self {
        Self::from_bits((1 << (signum - 1)) as u64).unwrap()
    }

    pub fn from_sigset(set: libc::sigset_t) -> Self {
        let mut raw_set = 0u64;
        for i in 0u64..=31u64 {
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

    pub fn wait(&self, ctx: &mut FizzleSingleton, timeout: Option<libc::timespec>) -> Option<libc::siginfo_t> {
        todo!()
    }

    pub fn set_signal_handler(&self, ctx: &mut FizzleSingleton, signal: libc::c_int, handler: libc::sigaction) {
        let mut state = ctx.acquire();
        let thread_id = thread::current().id();

        let new_handler = match handler.sa_sigaction {
            libc::SIG_DFL => SigCallback::Default,
            libc::SIG_IGN => SigCallback::Ignore,
            a if handler.sa_flags & libc::SA_SIGINFO > 0 => SigCallback::Action(unsafe { mem::transmute(a) }),
            a => SigCallback::Handler(unsafe { mem::transmute(a) }),
        };

        state.local.signals.get_mut(&thread_id).unwrap().handlers[signal as usize - 1] = new_handler;
        todo!()
    }

    pub fn get_signal_handler(&self, ctx: &mut FizzleSingleton) {
        todo!()
    }
}

#[derive(Clone, Debug)]
pub enum SigCallback {
    Default,
    Ignore,
    Handler(SigHandler),
    Action(SigAction),
}

impl Default for SigCallback {
    fn default() -> Self {
        Self::Default
    }
}

pub type SigHandler = unsafe extern "C" fn(libc::c_int);
pub type SigAction = unsafe extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void);

#[derive(Clone, Debug, Default)]
pub struct ThreadSignalInfo {
    pub mask: SignalSet,
    pub handlers: [SigCallback; 32],
}

impl ThreadSignalInfo {
    pub fn new() -> Self {
        // We need to block SIGPIPE as it can naturally occur during I/O
        let new_set = SignalSet::SIGPIPE.to_sigset();
        let mut old_set = SignalSet::empty().to_sigset();

        unsafe {
            assert_eq!(libc::pthread_sigmask(libc::SIG_SETMASK, ptr::addr_of!(new_set), ptr::addr_of_mut!(old_set)), 0);
        }

        Self {
            mask: SignalSet::from_sigset(old_set),
            handlers: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessSignalInfo {
    // According to `pthreads(7)`,, POSIX.1 specifies that threads of a process should all share
    // signal disposition.
    pub raised: SignalSet,
    // This is an Option so that we don't pre-allocate a bunch of `PolledInfo` instances if we
    // don't need them.
    pub polled: [Option<Rc<PolledId>>; 32],
}
