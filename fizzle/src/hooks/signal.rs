use std::mem;
use std::time::Duration;

use crate::errno::Errno;
use crate::handlers::process::{Pgid, Pid};
use crate::handlers::signal::*;
use crate::handlers::thread::Tid;
use crate::hook_macros;
use crate::scheduler::Scheduler;

// SIGSEGV, SIGBUS, SIGFPE and family can't be caught using `sigwait` or `signalfd`. But SIGCHLD can...

hook_macros::hook! {
    unsafe fn sigwait(
        set: *const libc::sigset_t,
        sig: *mut libc::c_int
    ) -> libc::c_int => fizzle_sigwait(ctx) {
        let signal_set = SignalSet::from_sigset(*set);

        crate::strace!("sigwait(set={:?}, sig={:?}) -> ...", signal_set, sig);

        match Scheduler::handle_event(&mut ctx, SignalWaitEvent::new(signal_set, None)) {
            Ok(raised_info) => {
                let siginfo = siginfo_t::from_raised(raised_info);

                if let Some(sig_mut) = unsafe { sig.as_mut() } {
                    *sig_mut = siginfo.si_signo;
                }

                crate::strace!("sigwait(set={:?}, sig={} ({:?})) -> 0", signal_set, siginfo.si_signo, sig);
                0
            },
            Err(_) => unreachable!("sigwait() cannot return error"),
        }
    }
}

hook_macros::hook! {
    unsafe fn sigwaitinfo(
        set: *const libc::sigset_t,
        info: *mut siginfo_t
    ) -> libc::c_int => fizzle_sigwaitinfo(ctx) {
        let signal_set = SignalSet::from_sigset(*set);

        crate::strace!("sigwaitinfo(set={:?}, info={:?}) -> ...", signal_set, info);

        match Scheduler::handle_event(&mut ctx, SignalWaitEvent::new(signal_set, None)) {
            Ok(raised_info) => {
                let siginfo = siginfo_t::from_raised(raised_info);

                let ret = siginfo.si_signo;
                if let Some(siginfo_mut) = unsafe { info.as_mut() } {
                    *siginfo_mut = siginfo;
                }

                crate::strace!("sigwaitinfo(set={:?}, info=({:?})) -> {}", signal_set, info, ret);
                ret
            },
            Err(e) => {
                crate::strace!("sigwaitinfo(set={:?}, info=({:?})) -> -1 ({})", signal_set, info, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sigtimedwait(
        set: *const libc::sigset_t,
        info: *mut siginfo_t,
        timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_sigtimedwait(ctx) {
        let signal_set = SignalSet::from_sigset(*set);
        let timeout = match unsafe { timeout.as_ref() } {
            Some(t) => {
                if t.tv_sec < 0 || t.tv_nsec < 0 {
                    crate::strace!("sigtimedwait(set={:?}, info={:?}, timeout={:?}) -> -1 (EINVAL)", signal_set, info, timeout);
                    Errno::EINVAL.set_errno();
                    return -1
                }

                Some(Duration::from_secs(t.tv_sec as u64) + Duration::from_nanos(t.tv_nsec as u64))
            },
            None => None,
        };

        crate::strace!("sigtimedwait(set={:?}, info={:?}, timeout={:?}) -> ...", signal_set, info, timeout);

        match Scheduler::handle_event(&mut ctx, SignalWaitEvent::new(signal_set, timeout)) {
            Ok(raised_info) => {
                let siginfo = siginfo_t::from_raised(raised_info);

                let ret = siginfo.si_signo;
                if let Some(siginfo_mut) = unsafe { info.as_mut() } {
                    *siginfo_mut = siginfo;
                }

                crate::strace!("sigtimedwait(set={:?}, info=({:?}), timeout={:?}) -> {}", signal_set, info, timeout, ret);
                ret
            },
            Err(e) => {
                crate::strace!("sigtimedwait(set={:?}, info=({:?}), timeout={:?}) -> -1 ({})", signal_set, info, timeout, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn signalfd(
        _fd: libc::c_int,
        _mask: *const libc::sigset_t,
        _flags: libc::c_int
    ) -> libc::c_int => fizzle_signalfd(_ctx) {
        unimplemented!("signalfd()")
    }
}

hook_macros::hook! {
    unsafe fn signal(
        signum: libc::c_int,
        handler: libc::sighandler_t
    ) -> libc::sighandler_t => fizzle_signal(ctx) {
        if signum <= 0 || signum > 32 || signum == libc::SIGKILL || signum == libc::SIGSTOP {
            Errno::EINVAL.set_errno();
            return libc::SIG_ERR
        }

        crate::strace!("signal(signum={}, handler={}) -> ...", signum, handler);

        let new_handler = match handler {
            libc::SIG_DFL => SigDisposition::Default,
            libc::SIG_IGN => SigDisposition::Ignore,
            _ => SigDisposition::Handler(unsafe { mem::transmute(handler) })
        };

        match Scheduler::handle_event(&mut ctx, SignalSetHandlerEvent::new(signum, Some(new_handler))) {
            Ok(old_handler) => {
                let out = match old_handler {
                    SigDisposition::Default => libc::SIG_DFL,
                    SigDisposition::Ignore => libc::SIG_IGN,
                    SigDisposition::Handler(h) => unsafe { mem::transmute(h) },
                    SigDisposition::Action(a) => unsafe { mem::transmute(a) },
                };

                crate::strace!("signal(signum={}, handler={}) -> {}", signum, handler, out);
                out
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn sigaction(
        signum: libc::c_int,
        act: *const libc::sigaction,
        oldact: *mut libc::sigaction
    ) -> libc::c_int => fizzle_sigaction(ctx) {
        if signum <= 0 || signum > 32 || signum == libc::SIGKILL || signum == libc::SIGSTOP {
            Errno::EINVAL.set_errno();
            return -1
        }

        crate::strace!("sigaction(signum={}, act={:?}, oldact={:?}) -> ...", signum, act, oldact);

        let new_handler = if let Some(act) = unsafe { act.as_ref() } {
            let flags = act.sa_flags;

            if flags & !libc::SA_SIGINFO > 0 {
                log::warn!("sigaction() had unsupported flags");
            }

            if !SignalSet::from_sigset(act.sa_mask).is_empty() {
                log::warn!("sigaction() sigmask unsupported");
            }

            Some(match act.sa_sigaction {
                libc::SIG_DFL => SigDisposition::Default,
                libc::SIG_IGN => SigDisposition::Ignore,
                handler if flags & libc::SA_SIGINFO > 0 => SigDisposition::Action(unsafe { mem::transmute(handler) }),
                handler => SigDisposition::Handler(unsafe { mem::transmute(handler) }),
            })

        } else {
            None
        };

        match Scheduler::handle_event(&mut ctx, SignalSetHandlerEvent::new(signum, new_handler)) {
            Ok(old_handler) => {
                if let Some(oldact) = unsafe { oldact.as_mut() } {
                    oldact.sa_sigaction = match old_handler {
                        SigDisposition::Default => libc::SIG_DFL,
                        SigDisposition::Ignore => libc::SIG_IGN,
                        SigDisposition::Handler(h) => unsafe { mem::transmute(h) },
                        SigDisposition::Action(a) => unsafe { mem::transmute(a) },
                    };

                    oldact.sa_flags = match old_handler {
                        SigDisposition::Handler(_) => libc::SA_SIGINFO,
                        _ => 0
                    };
                }

                crate::strace!("sigaction(signum={}, act={:?}, oldact={:?}) -> 0", signum, act, oldact);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn kill(
        pid: libc::pid_t,
        sig: libc::c_int
    ) -> libc::c_int => fizzle_kill(ctx) {
        crate::strace!("kill(pid={}, sig={}) -> ...", pid, sig);

        let target = match pid {
            1.. => SignalTarget::Pid(Pid::from_raw(pid)),
            0 => SignalTarget::CallingProcessGroup,
            -1 => SignalTarget::AllPermissive,
            ..=-2 => SignalTarget::Pgid(Pgid::from_raw(-pid)),
        };

        match Scheduler::handle_event(&mut ctx, SignalSendEvent::new(target, sig, None)) {
            Ok(()) => {
                crate::strace!("kill(pid={}, sig={}) -> 0", pid, sig);
                0
            },
            Err(e) => {
                crate::strace!("kill(pid={}, sig={}) -> -1 ({})", pid, sig, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn killpg(
        pgrp: libc::c_int,
        sig: libc::c_int
    ) -> libc::c_int => fizzle_killpg(ctx) {
        crate::strace!("killpg(pgrp={}, sig={}) -> ...", pgrp, sig);

        let target = match pgrp {
            1.. => SignalTarget::Pgid(Pgid::from_raw(pgrp)),
            0 => SignalTarget::CallingProcessGroup,
            ..=-1 => {
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, SignalSendEvent::new(target, sig, None)) {
            Ok(()) => {
                crate::strace!("killpg(pgrp={}, sig={}) -> 0", pgrp, sig);
                0
            },
            Err(e) => {
                crate::strace!("killpg(pgrp={}, sig={}) -> -1 ({})", pgrp, sig, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sigqueue(
        pid: libc::pid_t,
        sig: libc::c_int,
        value: sigval
    ) -> libc::c_int => fizzle_sigqueue(ctx) {
        crate::strace!("sigqueue(pid={}, sig={}, value=<union>) -> ...", pid, sig);

        let target = match pid {
            1.. => SignalTarget::Pid(Pid::from_raw(pid)),
            ..=-0 => {
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, SignalSendEvent::new(target, sig, Some(value))) {
            Ok(()) => {
                crate::strace!("sigqueue(pid={}, sig={}, value=<union>) -> 0", pid, sig);
                0
            },
            Err(e) => {
                crate::strace!("killpg(pgrp={}, sig={}, value=<union>) -> -1 ({})", pid, sig, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_kill(
        thread: libc::pthread_t,
        sig: libc::c_int
    ) -> libc::c_int => fizzle_pthread_kill(ctx) {
        crate::strace!("pthread_kill(thread={}, sig={}) -> ...", thread, sig);

        match Scheduler::handle_event(&mut ctx, SignalSendEvent::new(SignalTarget::Thread(thread), sig, None)) {
            Ok(()) => {
                crate::strace!("pthread_kill(thread={}, sig={}) -> 0", thread, sig);
                0
            },
            Err(e) => {
                crate::strace!("pthread_kill(thread={}, sig={}) -> -1 ({})", thread, sig, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn tgkill(
        tgid: libc::pid_t,
        tid: libc::pid_t,
        sig: libc::c_int
    ) -> libc::c_int => fizzle_tgkill(ctx) {
        crate::strace!("tgkill(tgid={}, tid={}, sig={}) -> ...", tgid, tid, sig);

        match Scheduler::handle_event(&mut ctx, SignalSendEvent::new(SignalTarget::Tid(Tid::from_raw(tid), Pid::from_raw(tgid)), sig, None)) {
            Ok(()) => {
                crate::strace!("tgkill(tgid={}, tid={}, sig={}) -> 0", tgid, tid, sig);
                0
            },
            Err(e) => {
                crate::strace!("tgkill(tgid={}, tid={}, sig={}) -> -1 ({})", tgid, tid, sig, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sigpending(
        set: *mut libc::sigset_t
    ) -> libc::c_int => fizzle_sigpending(ctx) {
        crate::strace!("sigpending(set={:?}) -> ...", set);

        match Scheduler::handle_event(&mut ctx, SignalGetPendingEvent) {
            Ok(raised_info) => {
                unsafe {
                    *set = raised_info.to_sigset();
                }

                crate::strace!("sigpending(set={:?} ({:?})) -> 0", raised_info, set);
                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn sigprocmask(
        how: libc::c_int,
        set: *const libc::sigset_t,
        oldset: *mut libc::sigset_t
    ) -> libc::c_int => fizzle_sigprocmask(ctx) {

        let op = match how {
            libc::SIG_BLOCK => SigmaskOp::Block,
            libc::SIG_SETMASK => SigmaskOp::Setmask,
            libc::SIG_UNBLOCK => SigmaskOp::Unblock,
            _ => {
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        let mask = match unsafe { set.as_ref() } {
            Some(m) => Some(SignalSet::from_sigset(*m)),
            None => None,
        };

        crate::strace!("sigprocmask(how={:?}, set={:?}, oldset={:?}) -> ...", op, set, oldset);

        match Scheduler::handle_event(&mut ctx, SignalSetSigmaskEvent::new(op, mask)) {
            Ok(raised_info) => {
                if let Some(old) = unsafe { oldset.as_mut() } {
                    *old = raised_info.to_sigset();
                }

                crate::strace!("sigprocmask(how={:?}, set={:?}, oldset={:?}) -> 0", op, set, oldset);
                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_sigmask(
        how: libc::c_int,
        set: *const libc::sigset_t,
        oldset: *mut libc::sigset_t
    ) -> libc::c_int => fizzle_pthread_sigmask(ctx) {

        let op = match how {
            libc::SIG_BLOCK => SigmaskOp::Block,
            libc::SIG_SETMASK => SigmaskOp::Setmask,
            libc::SIG_UNBLOCK => SigmaskOp::Unblock,
            _ => {
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        let mask = match unsafe { set.as_ref() } {
            Some(m) => Some(SignalSet::from_sigset(*m)),
            None => None,
        };

        crate::strace!("pthread_signask(how={:?}, set={:?}, oldset={:?}) -> ...", op, set, oldset);

        match Scheduler::handle_event(&mut ctx, SignalSetSigmaskEvent::new(op, mask)) {
            Ok(raised_info) => {
                if let Some(old) = unsafe { oldset.as_mut() } {
                    *old = raised_info.to_sigset();
                }

                crate::strace!("pthread_sigmask(how={:?}, set={:?}, oldset={:?}) -> 0", op, set, oldset);
                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn sigsuspend(
        mask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_sigsuspend(ctx) {
        let sigmask = SignalSet::from_sigset(unsafe { *mask });

        crate::strace!("sigsuspend(mask={:?} ({:?})) -> ...", sigmask, mask);

        match Scheduler::handle_event(&mut ctx, SignalSuspendEvent::new(sigmask)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("sigsuspend(mask={:?} ({:?})) -> -1 ({})", sigmask, mask, e);
                e.set_errno();
                -1
            }
        }
    }
}
