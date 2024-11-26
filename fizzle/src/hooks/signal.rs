use std::time::Duration;
use std::{mem, thread};

use crate::errno::Errno;
use crate::handlers::polled::PolledInfo;
use crate::handlers::signal::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

// TODO: SIGKILL and SIGSTOP need to be handled specially

// TODO: signals specifically targeting threads...

// SIGSEGV, SIGBUS, SIGFPE and family can't be caught using `sigwait` or `signalfd`. But SIGCHLD can...

// TODO: need to DRY out this code...

// TODO: sigsetjmp, siglongjmp

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


        /*
        if !info.is_null() {
            libc::memset(info as *mut libc::c_void, 0, mem::size_of::<libc::siginfo_t>());
        }

        let signal_set = SignalSet::from_sigset(*set);
        let mut state = ctx.acquire();

        let process_id = state.local.process_id;
        let poller = state.new_poller();

        let signals = state.global.processes.get_mut(&process_id).unwrap();
        let ready = signal_set.intersection(signals.raised);

        for flag in signal_set.iter() {
            let signal_value = flag.lowest_signal_value();

            let signals = state.global.processes.get_mut(&process_id).unwrap();

            let polled = match signals.polled[signal_value as usize].clone() {
                None if ready.contains(flag) => {
                    // No pollers were waiting on the signal--immediately return
                    signals.raised = signals.raised.difference(flag);

                    if !info.is_null() {
                        (*info).si_signo = signal_value;
                    }

                    return signal_value
                }
                None => {
                    let polled = state.global.polled_events.allocate(PolledInfo::default()).unwrap();
                    let signals = state.global.processes.get_mut(&process_id).unwrap();
                    signals.polled[signal_value as usize].replace(polled.clone());
                    polled
                }
                Some(polled) => polled,
            };

            if ready.contains(flag) && state.global.polled_events.get(&polled).unwrap().pollers.is_empty() {
                // No other pollers were waiting on the signal--immediately return
                let signals = state.global.processes.get_mut(&process_id).unwrap();
                signals.raised = signals.raised.difference(flag);

                if !info.is_null() {
                    (*info).si_signo = signal_value;
                }

                return signal_value
            }

            state.register_poller(poller.clone(), polled);
        }

        drop(state);

        // Wait for one of the signals to become available
        poller.poll(&mut ctx);

        let mut state = ctx.acquire();
        state.delete_poller(poller);

        let signals = state.global.processes.get_mut(&process_id).unwrap();
        let ready = signal_set.intersection(signals.raised);
        let first = ready.iter().next().unwrap(); // Polling returned, so there *must* be one ready

        // Consume the selected signal
        signals.raised = signals.raised.difference(first);
        let signal_value = first.lowest_signal_value();
        let signal_polled = signals.polled[signal_value as usize].clone().unwrap();

        drop(state);

        signal_polled.lower_polled(&mut ctx);

        // Return the selected signal
        if !info.is_null() {
            (*info).si_signo = signal_value;
        }

        return signal_value
        */
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

        crate::strace!("sigwaitinfo(set={:?}, info={:?}, timeout={:?}) -> ...", signal_set, info, timeout);

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

        /*
        ctx.yield_thread();
        // TODO: not all of siginfo_t's fields are filled here...

        if !info.is_null() {
            libc::memset(info as *mut libc::c_void, 0, mem::size_of::<libc::siginfo_t>());
        }

        let signal_set = SignalSet::from_sigset(*set);
        let mut state = ctx.acquire();

        let process_id = state.local.process_id;
        let poller_id = state.new_poller();

        let signals = state.global.processes.get_mut(&process_id).unwrap();
        let ready = signal_set.intersection(signals.raised);

        for flag in signal_set.iter() {
            let signal_value = flag.lowest_signal_value();

            let signals = state.global.processes.get_mut(&process_id).unwrap();

            let polled = match signals.polled[signal_value as usize].clone() {
                None if ready.contains(flag) => {
                    // No pollers were waiting on the signal--immediately return
                    signals.raised = signals.raised.difference(flag);

                    if !info.is_null() {
                        (*info).si_signo = signal_value;
                    }

                    return signal_value
                }
                None => {
                    let polled = state.global.polled_events.allocate(PolledInfo::default()).unwrap();
                    let signals = state.global.processes.get_mut(&process_id).unwrap();
                    signals.polled[signal_value as usize].replace(polled.clone());
                    polled
                }
                Some(polled) => polled,
            };

            if ready.contains(flag) && state.global.polled_events.get(&polled).unwrap().pollers.is_empty() {
                // No other pollers were waiting on the signal--immediately return
                let signals = state.global.processes.get_mut(&process_id).unwrap();
                signals.raised = signals.raised.difference(flag);

                if !info.is_null() {
                    (*info).si_signo = signal_value;
                }

                return signal_value
            }

            state.register_poller(poller_id.clone(), polled);
        }

        // TODO: any timeout other than 0 leads to indefinite blocking...
        if !timeout.is_null() && (*timeout).tv_sec == 0 && (*timeout).tv_nsec == 0 {
            state.delete_poller(poller_id);
            *libc::__errno_location() = libc::EAGAIN;
            return -1
        }

        drop(state);

        // Wait for one of the signals to become available
        poller_id.poll(&mut ctx);

        let mut state = ctx.acquire();
        state.delete_poller(poller_id);

        let signals = state.global.processes.get_mut(&process_id).unwrap();
        let ready = signal_set.intersection(signals.raised);
        let first = ready.iter().next().unwrap(); // Polling returned, so there *must* be one ready

        // Consume the selected signal
        signals.raised = signals.raised.difference(first);
        let signal_value = first.lowest_signal_value();
        let signal_polled = signals.polled[signal_value as usize].clone().unwrap();

        drop(state);

        signal_polled.lower_polled(&mut ctx);

        // Return the selected signal
        if !info.is_null() {
            (*info).si_signo = signal_value;
        }

        return signal_value
        */
    }
}

hook_macros::hook! {
    unsafe fn signal(
        signum: libc::c_int,
        handler: libc::sighandler_t
    ) -> libc::sighandler_t => fizzle_signal(ctx) {
        if signum > 32 {
            *libc::__errno_location() = libc::EINVAL;
            return libc::SIG_ERR
        }

        let mut state = ctx.acquire();

        let signals = state.local.signals.get_mut(&thread::current().id()).unwrap();

        let prev_handler = match signals.handlers[(signum - 1) as usize] {
            SigDisposition::Default => libc::SIG_DFL,
            SigDisposition::Ignore => libc::SIG_IGN,
            SigDisposition::Handler(handler) => handler as usize,
            SigDisposition::Action(action) => action as usize,
        };

        let new_handler = match handler {
            libc::SIG_DFL => SigDisposition::Default,
            libc::SIG_IGN => SigDisposition::Ignore,
            h => SigDisposition::Handler(mem::transmute(h)),
        };

        signals.handlers[(signum - 1) as usize] = new_handler;

        prev_handler
    }
}

hook_macros::hook! {
    unsafe fn kill(
        _pid: libc::pid_t,
        _sig: libc::c_int
    ) -> libc::c_int => fizzle_kill(_ctx) {
        unimplemented!("kill()")
    }
}

hook_macros::hook! {
    unsafe fn killpg(
        _pgrp: libc::c_int,
        _sig: libc::c_int
    ) -> libc::c_int => fizzle_killpg(_ctx) {
        unimplemented!("killpg()")
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
    unsafe fn sigaction(
        signum: libc::c_int,
        act: *const libc::sigaction,
        oldact: *mut libc::sigaction
    ) -> libc::c_int => fizzle_sigaction(ctx) {
        if signum > 32 {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        }

        let mut state = ctx.acquire();

        let signals = state.local.signals.get_mut(&thread::current().id()).unwrap();

        if !oldact.is_null() {
            let (prev_action, prev_flags) = match signals.handlers[(signum - 1) as usize] {
                SigDisposition::Default => (libc::SIG_DFL, 0),
                SigDisposition::Ignore => (libc::SIG_IGN, 0),
                SigDisposition::Handler(handler) => (handler as usize, 0),
                SigDisposition::Action(action) => (action as usize, libc::SA_SIGINFO),
            };

            let prev_action = libc::sigaction {
                sa_sigaction: prev_action,
                sa_mask: SignalSet::from_signum(signum).to_sigset(),
                sa_flags: prev_flags, // TODO: doesn't preserve other flags
                sa_restorer: None
            };

            *oldact = prev_action;
        }

        if !act.is_null() {
            let new_handler = match (*act).sa_sigaction {
                libc::SIG_DFL => SigDisposition::Default,
                libc::SIG_IGN => SigDisposition::Ignore,
                a if (*act).sa_flags & libc::SA_SIGINFO > 0 => SigDisposition::Action(mem::transmute(a)),
                a => SigDisposition::Handler(mem::transmute(a)),
            };

            signals.handlers[(signum - 1) as usize] = new_handler;
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sigpending(
        set: *mut libc::sigset_t
    ) -> libc::c_int => fizzle_sigpending(ctx) {

        let state = ctx.acquire();
        let process_id = state.local.process_id;
        *set = state.global.processes.get(&process_id).unwrap().raised.to_sigset();

        0
    }
}

hook_macros::hook! {
    unsafe fn sigsuspend(
        mask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_sigsuspend(ctx) {
        let mut state = ctx.acquire();
        let thread_id = thread::current().id();
        let raised_mask = &mut state.local.signals.get_mut(&thread_id).unwrap().mask;
        let old_raised = *raised_mask;
        *raised_mask = SignalSet::from_sigset(*mask);

        todo!(); // implement waiting for sigmask here

        let raised_mask = &mut state.local.signals.get_mut(&thread_id).unwrap().mask;
        *raised_mask = old_raised;

        0
    }
}

hook_macros::hook! {
    unsafe fn sigprocmask(
        how: libc::c_int,
        set: *const libc::sigset_t,
        oldset: *mut libc::sigset_t
    ) -> libc::c_int => fizzle_sigprocmask(ctx) {
        // Behaves identically to pthread_sigmask
        let mut state = ctx.acquire();
        let thread_id = thread::current().id();

        if !oldset.is_null() {
            *oldset = state.local.signals.get(&thread_id).unwrap().mask.to_sigset();
        }

        if !set.is_null() {
            match how {
                libc::SIG_SETMASK => state.local.signals.get_mut(&thread_id).unwrap().mask = SignalSet::from_sigset(*set),
                libc::SIG_BLOCK => state.local.signals.get_mut(&thread_id).unwrap().mask |= SignalSet::from_sigset(*set),
                libc::SIG_UNBLOCK => state.local.signals.get_mut(&thread_id).unwrap().mask &= !SignalSet::from_sigset(*set),
                _ => {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sigqueue(
        pid: libc::pid_t,
        sig: libc::c_int,
        value: sigval
    ) -> libc::c_int => fizzle_sigqueue(ctx) {
        todo!("sigqueue unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn pthread_kill(
        _thread: libc::pthread_t,
        _sig: libc::c_int
    ) -> libc::c_int => fizzle_pthread_kill(_ctx) {

        panic!("`pthread_kill` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn pthread_sigmask(
        how: libc::c_int,
        set: *const libc::sigset_t,
        oldset: *mut libc::sigset_t
    ) -> libc::c_int => fizzle_pthread_sigmask(ctx) {
        let mut state = ctx.acquire();
        let thread_id = thread::current().id();

        if !oldset.is_null() {
            *oldset = state.local.signals.get(&thread_id).unwrap().mask.to_sigset();
        }

        if !set.is_null() {
            match how {
                libc::SIG_SETMASK => state.local.signals.get_mut(&thread_id).unwrap().mask = SignalSet::from_sigset(*set),
                libc::SIG_BLOCK => state.local.signals.get_mut(&thread_id).unwrap().mask |= SignalSet::from_sigset(*set),
                libc::SIG_UNBLOCK => state.local.signals.get_mut(&thread_id).unwrap().mask &= !SignalSet::from_sigset(*set),
                _ => {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn tgkill(
        _tgid: libc::pid_t,
        _tid: libc::pid_t,
        _sig: libc::c_int
    ) -> libc::c_int => fizzle_tgkill(_ctx) {
        panic!("`tgkill` unimplemented")
    }
}
