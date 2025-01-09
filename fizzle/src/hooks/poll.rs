use std::slice;
use std::time::Duration;

use crate::errno::Errno;
use crate::handlers::descriptor::Descriptor;
use crate::handlers::poller::*;
use crate::handlers::signal::SignalSet;
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn select(
        nfds: libc::c_int,
        readfds: *mut libc::fd_set,
        writefds: *mut libc::fd_set,
        exceptfds: *mut libc::fd_set,
        timeout: *const libc::timeval
    ) -> libc::c_int => fizzle_select(ctx) {
        crate::strace!("select(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}) -> ...", nfds, readfds, writefds, exceptfds, timeout);

        let Ok(nfds) = nfds.try_into() else {
            crate::strace!("select(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}) -> -1 (EINVAL)", nfds, readfds, writefds, exceptfds, timeout);
            Errno::EINVAL.set_errno();
            return -1
        };

        if nfds > libc::FD_SETSIZE {
            log::error!("select() buffer overflow in fd_set size");
            panic!("select() buffer overflow in fd_set size")
        }

        let duration = if timeout.is_null() {
            None
        } else {
            if (*timeout).tv_sec < 0 || (*timeout).tv_usec < 0 {
                crate::strace!("select(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}) -> -1 (EINVAL)", nfds, readfds, writefds, exceptfds, timeout);
                Errno::EINVAL.set_errno();
                return -1
            }

            Some(Duration::from_secs((*timeout).tv_sec as u64) + Duration::from_micros((*timeout).tv_usec as u64))
        };

        let select_event = SelectEvent::new(
            nfds,
            readfds.as_mut(),
            writefds.as_mut(),
            exceptfds.as_mut(),
            duration,
            None
        );

        match Scheduler::handle_event(&mut ctx, select_event) {
            Ok(count) => {
                crate::strace!("select(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}) -> {}", nfds, readfds, writefds, exceptfds, duration, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("select(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}) -> -1 ({})", nfds, readfds, writefds, exceptfds, duration, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pselect(
        nfds:libc::c_int,
        readfds: *mut libc::fd_set,
        writefds: *mut libc::fd_set,
        exceptfds: *mut libc::fd_set,
        timeout: *const libc::timespec,
        sigmask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_pselect(ctx) {
        crate::strace!("pselect(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}, sigmask={:?}) -> -1 (EINVAL)", nfds, readfds, writefds, exceptfds, timeout, sigmask);

        let masked_set = if sigmask.is_null() {
            None
        } else {
            Some(SignalSet::from_sigset(*sigmask))
        };

        let Ok(nfds) = nfds.try_into() else {
            crate::strace!("pselect(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}, sigmask={:?}) -> -1 (EINVAL)", nfds, readfds, writefds, exceptfds, timeout, masked_set);
            Errno::EINVAL.set_errno();
            return -1
        };

        if nfds > libc::FD_SETSIZE {
            log::error!("pselect() buffer overflow in fd_set size");
            panic!("pselect() buffer overflow in fd_set size")
        }

        let duration = if timeout.is_null() {
            None
        } else {
            if (*timeout).tv_sec < 0 || (*timeout).tv_nsec < 0 {
                crate::strace!("pselect(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}, sigmask={:?}) -> -1 (EINVAL)", nfds, readfds, writefds, exceptfds, timeout, masked_set);
                Errno::EINVAL.set_errno();
                return -1
            }

            Some(Duration::from_secs((*timeout).tv_sec as u64) + Duration::from_nanos((*timeout).tv_nsec as u64))
        };

        let select_event = SelectEvent::new(
            nfds,
            readfds.as_mut(),
            writefds.as_mut(),
            exceptfds.as_mut(),
            duration,
            masked_set,
        );

        match Scheduler::handle_event(&mut ctx, select_event) {
            Ok(count) => {
                crate::strace!("pselect(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}, sigmask={:?}) -> {}", nfds, readfds, writefds, exceptfds, duration, masked_set, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("pselect(nfds={}, readfds={:?}, writefds={:?}, exceptfds={:?}, timeout={:?}, sigmask={:?}) -> -1 ({})", nfds, readfds, writefds, exceptfds, duration, masked_set, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn poll(
        fds: *mut libc::pollfd,
        nfds: libc::nfds_t,
        timeout: libc::c_int
    ) -> libc::c_int => fizzle_poll(ctx) {
        let duration = match timeout {
            ..=-1 => None,
            0.. => Some(Duration::from_millis(timeout as u64)),
        };

        crate::strace!("poll(fds={:?}, nfds={}, timeout={:?}) -> ...", fds, nfds, duration);

        let fd_info = unsafe {
            slice::from_raw_parts_mut(fds, nfds.try_into().unwrap())
        };

        match Scheduler::handle_event(&mut ctx, PollEvent::new(fd_info, duration, None)) {
            Ok(count) => {
                crate::strace!("poll(fds={:?}, nfds={}, timeout={:?}) -> {}", fds, nfds, duration, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("poll(fds={:?}, nfds={}, timeout={:?}) -> -1 ({})", fds, nfds, duration, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn ppoll(
        fds: *mut libc::pollfd,
        nfds: libc::nfds_t,
        tmo_p: *const libc::timespec,
        sigmask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_ppoll(ctx) {
        crate::strace!("ppoll(fds={:?}, nfds={}, tmo_p={:?}, sigmask={:?}) -> ...", fds, nfds, tmo_p, sigmask);

        let duration = if tmo_p.is_null() {
            None
        } else {
            Some(Duration::from_secs((*tmo_p).tv_sec as u64) + Duration::from_nanos((*tmo_p).tv_nsec as u64))
        };

        let masked_set = if sigmask.is_null() {
            None
        } else {
            Some(SignalSet::from_sigset(*sigmask))
        };

        let fd_info = unsafe {
            slice::from_raw_parts_mut(fds, nfds.try_into().unwrap())
        };

        match Scheduler::handle_event(&mut ctx, PollEvent::new(fd_info, duration, masked_set)) {
            Ok(count) => {
                crate::strace!("ppoll(fds={:?}, nfds={}, tmo_p={:?} ({:?}), sigmask={:?} ({:?})) -> {}", fds, nfds, tmo_p, duration, sigmask, masked_set, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("ppoll(fds={:?}, nfds={}, tmo_p={:?} ({:?}), sigmask={:?} ({:?})) -> -1 ({})", fds, nfds, tmo_p, duration, sigmask, masked_set, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn epoll_create(
        size: libc::c_int
    ) -> libc::c_int => fizzle_epoll_create(ctx) {
        crate::strace!("epoll_create(size={}) -> ...", size);

        match Scheduler::handle_event(&mut ctx, EpollCreateEvent::new(false)) {
            Ok(fd) => {
                let fd = fd.as_raw_fd();
                crate::strace!("epoll_create(size={}) -> {}", size, fd);
                fd
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn epoll_create1(
        flags: libc::c_int
    ) -> libc::c_int => fizzle_epoll_create1(ctx) {
        crate::strace!("epoll_create1(flags={}) -> ...", flags);
        let cloexec = flags & libc::EPOLL_CLOEXEC > 0;

        match Scheduler::handle_event(&mut ctx, EpollCreateEvent::new(cloexec)) {
            Ok(fd) => {
                let fd = fd.as_raw_fd();
                crate::strace!("epoll_create1(flags={}) -> {}", flags, fd);
                fd
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn epoll_ctl(
        epfd: libc::c_int,
        op: libc::c_int,
        fd: libc::c_int,
        event: *mut libc::epoll_event
    ) -> libc::c_int => fizzle_epoll_ctl(ctx) {
        crate::strace!("epoll_ctl(epfd={}, op={}, fd={}, event={:?}) -> ...", epfd, op, fd, event);

        let epoll_descriptor = Descriptor::from_raw_fd(epfd);
        let target_descriptor = Descriptor::from_raw_fd(fd);

        let operation = match op {
            libc::EPOLL_CTL_ADD => EpollOperation::Add(*event),
            libc::EPOLL_CTL_DEL => EpollOperation::Delete,
            libc::EPOLL_CTL_MOD => EpollOperation::Modify(*event),
            _ => {
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, EpollCtlEvent::new(epoll_descriptor, operation, target_descriptor)) {
            Ok(()) => {
                crate::strace!("epoll_ctl(epfd={}, op={}, fd={}, event={:?}) -> 0", epfd, op, fd, event);
                fd
            },
            Err(e) => {
                crate::strace!("epoll_ctl(epfd={}, op={}, fd={}, event={:?}) -> -1 ({})", epfd, op, fd, event, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn epoll_wait(
        epfd: libc::c_int,
        events: *mut libc::epoll_event,
        maxevents: libc::c_int,
        timeout: libc::c_int
    ) -> libc::c_int => fizzle_epoll_wait(ctx) {
        let duration = match timeout {
            ..=-1 => None,
            0.. => Some(Duration::from_millis(timeout as u64)),
        };

        crate::strace!("epoll_wait(epfd={}, events={:?}, maxevents={}, timeout={:?}) -> ...", epfd, events, maxevents, duration);

        if maxevents <= 0 {
            crate::strace!("epoll_wait(epfd={}, events={:?}, maxevents={}, timeout={:?}) -> -1 (EINVAL)", epfd, events, maxevents, duration);
            Errno::EINVAL.set_errno();
            return -1
        }

        let ep_descriptor = Descriptor::from_raw_fd(epfd);

        let event_info = slice::from_raw_parts_mut(events, maxevents as usize);
        for event in event_info.iter_mut() {
            *event = libc::epoll_event { events: 0, u64: 0 }
        }

        match Scheduler::handle_event(&mut ctx, EpollWaitEvent::new(ep_descriptor, event_info, duration, None)) {
            Ok(count) => {
                crate::strace!("epoll_wait(epfd={}, events={:?}, maxevents={}, timeout={:?}) -> {})", epfd, events, maxevents, duration, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("epoll_wait(epfd={}, events={:?}, maxevents={}, timeout={:?}) -> -1 ({})", epfd, events, maxevents, duration, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn epoll_pwait(
        epfd: libc::c_int,
        events: *mut libc::epoll_event,
        maxevents: libc::c_int,
        timeout: libc::c_int,
        sigmask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_epoll_pwait(ctx) {
        let duration = match timeout {
            ..=-1 => None,
            0.. => Some(Duration::from_millis(timeout as u64)),
        };

        crate::strace!("epoll_pwait(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> ...", epfd, events, maxevents, duration, sigmask);

        if maxevents <= 0 {
            crate::strace!("epoll_pwait(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> -1 (EINVAL)", epfd, events, maxevents, duration, sigmask);
            Errno::EINVAL.set_errno();
            return -1
        }

        let block_set = if sigmask.is_null() {
            None
        } else {
            Some(SignalSet::from_sigset(*sigmask))
        };

        let ep_descriptor = Descriptor::from_raw_fd(epfd);

        let event_info = slice::from_raw_parts_mut(events, maxevents as usize);
        for event in event_info.iter_mut() {
            *event = libc::epoll_event { events: 0, u64: 0 }
        }

        match Scheduler::handle_event(&mut ctx, EpollWaitEvent::new(ep_descriptor, event_info, duration, block_set)) {
            Ok(count) => {
                crate::strace!("epoll_pwait(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> {}", epfd, events, maxevents, duration, block_set, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("epoll_pwait(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> -1 ({})", epfd, events, maxevents, duration, block_set, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn epoll_pwait2(
        epfd: libc::c_int,
        events: *mut libc::epoll_event,
        maxevents: libc::c_int,
        timeout: *const libc::timespec,
        sigmask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_epoll_pwait2(ctx) {
        crate::strace!("epoll_pwait2(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> ...", epfd, events, maxevents, timeout, sigmask);

        let duration = if timeout.is_null() {
            None
        } else {
            if (*timeout).tv_sec < 0 || (*timeout).tv_nsec < 0 {
                Errno::EINVAL.set_errno();
                return -1
            }

            Some(Duration::from_secs((*timeout).tv_sec as u64) + Duration::from_nanos((*timeout).tv_nsec as u64))
        };

        if maxevents <= 0 {
            crate::strace!("epoll_pwait2(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> -1 (EINVAL)", epfd, events, maxevents, duration, sigmask);
            Errno::EINVAL.set_errno();
            return -1
        }

        let block_set = if sigmask.is_null() {
            None
        } else {
            Some(SignalSet::from_sigset(*sigmask))
        };

        let ep_descriptor = Descriptor::from_raw_fd(epfd);

        let event_info = slice::from_raw_parts_mut(events, maxevents as usize);
        for event in event_info.iter_mut() {
            *event = libc::epoll_event { events: 0, u64: 0 }
        }

        match Scheduler::handle_event(&mut ctx, EpollWaitEvent::new(ep_descriptor, event_info, duration, block_set)) {
            Ok(count) => {
                crate::strace!("epoll_pwait2(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> {}", epfd, events, maxevents, duration, block_set, count);
                count as libc::c_int
            },
            Err(e) => {
                crate::strace!("epoll_pwait2(epfd={}, events={:?}, maxevents={}, timeout={:?}, sigmask={:?}) -> -1 ({})", epfd, events, maxevents, duration, block_set, e);
                e.set_errno();
                -1
            },
        }
    }
}
