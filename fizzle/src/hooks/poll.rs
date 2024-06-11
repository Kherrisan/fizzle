
use std::{collections::HashMap, os::fd::RawFd, ptr};

use fxhash::FxBuildHasher;

use crate::{hook_macros, state};
use crate::state::backend::{ConnectedBackend, ConnectionlessBackend, FileBackend, StdioBackend};
use crate::state::{EpollDirection, EpollInfo, EpollInterest, FizzState, PolledStatus, SocketState};
use crate::state::fd::{FdInfo, FdResource};
use crate::state::identifiers::{DescriptorId, PolledId};

/// Polled for read() operations
pub fn fd_to_pollin(ctx: &mut FizzState, fd: RawFd) -> PolledStatus {
    let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
        return PolledStatus::BadFd
    };
    match fd_info.resource {
        FdResource::Epoll(_) => panic!("polling an epoll descriptor not supported"),
        FdResource::Directory(_) => PolledStatus::NotPollable,
        FdResource::File(file_id) => {
            match ctx.global.files.get(file_id).unwrap() {
                FileBackend::Passthrough => PolledStatus::ImmediatelyPollable, // TODO: should we `poll()` here instead?
                FileBackend::Peered(_) => unreachable!(),
                FileBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.read_polled),
                FileBackend::Plugin(plugin_id) => {
                    let plugin_id = *plugin_id;
                    PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().read_polled)
                }
                FileBackend::Sink => PolledStatus::NotPollable,
                FileBackend::NullSink => PolledStatus::ImmediatelyPollable,
                FileBackend::Fuzz => PolledStatus::Pollable(ctx.global.fuzz_endpoints.get(&FdResource::File(file_id)).unwrap().read_polled),
            }
        },
        FdResource::MessageQueue(_) => todo!(),
        FdResource::Pipe(pipe_id) => PolledStatus::Pollable(ctx.global.pipes.get(pipe_id).unwrap().read_polled),
        FdResource::Stdin => match ctx.global.stdio {
            StdioBackend::Passthrough => unreachable!(),
            StdioBackend::Peered(_) => unreachable!(),
            StdioBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.read_polled),
            StdioBackend::Plugin(plugin_id) => PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().read_polled),
            StdioBackend::Sink => PolledStatus::NotPollable,
            StdioBackend::NullSink => PolledStatus::ImmediatelyPollable,
            StdioBackend::Fuzz => PolledStatus::Pollable(ctx.global.fuzz_endpoints.get(&FdResource::Stdin).unwrap().read_polled),
        }
        FdResource::Stdout => PolledStatus::NotPollable,
        FdResource::Stderr => PolledStatus::NotPollable,
        FdResource::Socket(socket_id) => match ctx.global.sockets.get(socket_id).unwrap() {
            SocketState::Connectionless(connectionless) => match connectionless.backend {
                ConnectionlessBackend::Passthrough => unreachable!(),
                ConnectionlessBackend::Peered(regular) => PolledStatus::Pollable(regular.read_polled),
                ConnectionlessBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.read_polled),
                ConnectionlessBackend::Plugin(plugin_id) => PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().read_polled),
                ConnectionlessBackend::Sink => PolledStatus::NotPollable,
                ConnectionlessBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::Fuzz => PolledStatus::Pollable(ctx.global.fuzz_endpoints.get(&FdResource::Socket(socket_id)).unwrap().read_polled),
            },
            SocketState::Unassociated(_) => PolledStatus::NotPollable,
            SocketState::Server(server) => PolledStatus::Pollable(server.ready_to_connect),
            SocketState::PendingConnection(_) => PolledStatus::NotPollable,
            SocketState::Connecting(_) => PolledStatus::NotPollable, // Need to select for writing, not reading
            SocketState::Connected(connected) => match connected.backend {
                ConnectedBackend::Passthrough => unreachable!(),
                ConnectedBackend::Peered(regular) => PolledStatus::Pollable(regular.read_polled),
                ConnectedBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.read_polled),
                ConnectedBackend::Plugin(plugin_id) => PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().read_polled),
                ConnectedBackend::Sink => PolledStatus::NotPollable,
                ConnectedBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::Fuzz => PolledStatus::Pollable(ctx.global.fuzz_endpoints.get(&FdResource::Socket(socket_id)).unwrap().read_polled),
            },
            // SocketState::Error => PolledStatus::ImmediatelyPollable,
        }
    }
}

pub fn fd_to_pollout(ctx: &mut FizzState, fd: RawFd) -> PolledStatus {
    let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
        return PolledStatus::BadFd
    };
    match fd_info.resource {
        FdResource::Epoll(_) => panic!("polling an epoll descriptor not supported"),
        FdResource::Directory(_) => PolledStatus::NotPollable,
        FdResource::File(file_id) => {
            match ctx.global.files.get(file_id).unwrap() {
                FileBackend::Passthrough => PolledStatus::ImmediatelyPollable, // TODO: should we `poll()` here instead?
                FileBackend::Peered(_) => unreachable!(), 
                FileBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.write_polled),
                FileBackend::Plugin(plugin_id) => {
                    let plugin_id = *plugin_id;
                    PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().write_polled)
                }
                FileBackend::Sink => PolledStatus::ImmediatelyPollable,
                FileBackend::NullSink => PolledStatus::ImmediatelyPollable,
                FileBackend::Fuzz => PolledStatus::ImmediatelyPollable,
            }
        },
        FdResource::MessageQueue(_) => todo!(),
        FdResource::Pipe(pipe_id) => {
            if let Some(peer_id) = ctx.global.pipes.get(pipe_id).unwrap().peer {
                PolledStatus::Pollable(ctx.global.pipes.get(peer_id).unwrap().write_polled)
            } else {
                PolledStatus::ImmediatelyPollable
            }
        },
        FdResource::Stdin => PolledStatus::NotPollable,
        FdResource::Stdout => match ctx.global.stdio {
            StdioBackend::Passthrough => unreachable!(),
            StdioBackend::Peered(_) => unreachable!(),
            StdioBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.write_polled),
            StdioBackend::Plugin(plugin_id) => PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().write_polled),
            StdioBackend::Sink => PolledStatus::ImmediatelyPollable,
            StdioBackend::NullSink => PolledStatus::ImmediatelyPollable,
            StdioBackend::Fuzz => PolledStatus::ImmediatelyPollable,
        }
        FdResource::Stderr => PolledStatus::NotPollable,
        FdResource::Socket(socket_id) => match ctx.global.sockets.get(socket_id).unwrap() {
            SocketState::Connectionless(connectionless) => match connectionless.backend {
                ConnectionlessBackend::Passthrough => unreachable!(),
                ConnectionlessBackend::Peered(_) => PolledStatus::ImmediatelyPollable, // A connectionless socket can always `send()` TODO: ??
                ConnectionlessBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.write_polled),
                ConnectionlessBackend::Plugin(plugin_id) => PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().write_polled),
                ConnectionlessBackend::Sink => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::Fuzz => PolledStatus::ImmediatelyPollable,
            },
            SocketState::Unassociated(_) => PolledStatus::NotPollable,
            SocketState::Server(_) => PolledStatus::NotPollable, // Need to select for reading, not writing
            SocketState::PendingConnection(_) => PolledStatus::NotPollable,
            SocketState::Connecting(connecting) => PolledStatus::Pollable(connecting.connect_polled),
            SocketState::Connected(connected) => match connected.backend {
                ConnectedBackend::Passthrough => unreachable!(),
                ConnectedBackend::Peered(peered) => {
                    if let Some(peer_id) = peered.peer {
                        let SocketState::Connected(conn) = ctx.global.sockets.get(peer_id).unwrap() else {
                            panic!()
                        };

                        match &conn.backend {
                            ConnectedBackend::Peered(peer_info) => PolledStatus::Pollable(peer_info.write_polled),
                            _ => panic!(),
                        }

                    } else {
                        PolledStatus::ImmediatelyPollable // The next `write()` call will return 0
                    }
                }
                ConnectedBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.write_polled),
                ConnectedBackend::Plugin(plugin_id) => PolledStatus::Pollable(ctx.global.plugins.get(plugin_id).unwrap().write_polled),
                ConnectedBackend::Sink => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::Fuzz => PolledStatus::ImmediatelyPollable,
            },
            // SocketState::Error => PolledStatus::ImmediatelyPollable,
        }
    }
}


hook_macros::hook! {
    unsafe fn select(
        nfds: libc::c_int,
        readfds: *mut libc::fd_set,
        writefds: *mut libc::fd_set,
        exceptfds: *mut libc::fd_set,
        timeout: *const libc::timeval
    ) -> libc::c_int => fizzle_select(ctx) {
        drop(ctx);
        let tmo = libc::timespec {
            tv_sec: (*timeout).tv_sec,
            tv_nsec: (*timeout).tv_usec * 1000,
        };

        fizzle_pselect(nfds, readfds, writefds, exceptfds, ptr::addr_of!(tmo), ptr::null())
    }
}

hook_macros::hook! {
    unsafe fn pselect(
        nfds: libc::c_int,
        readfds: *mut libc::fd_set,
        writefds: *mut libc::fd_set,
        exceptfds: *mut libc::fd_set,
        timeout: *const libc::timespec,
        sigmask: *const libc::sigset_t
    ) -> libc::c_int => fizzle_pselect(ctx) {

        // TODO: we just ignore the signal mask for now
        // this may produce undefined behavior
        if !sigmask.is_null() {
            crate::report_strict_failure("fizzle internal error--sigmask unsupported for ppoll")
        }

        let mut total_ready = 0;

        // "Exceptional conditions" never happen in fizzle.
        if !exceptfds.is_null() {
            libc::FD_ZERO(exceptfds);
        }

        let mut read_pollers = HashMap::with_hasher(FxBuildHasher::default());
        let mut write_pollers = HashMap::with_hasher(FxBuildHasher::default());

        for fd in 0..nfds {
            if !readfds.is_null() {
                if libc::FD_ISSET(fd, readfds) {
                    match fd_to_pollin(&mut ctx, fd) {
                        PolledStatus::Pollable(polled_id) => {
                            if !ctx.polled_is_ready(polled_id) {
                                libc::FD_CLR(fd, readfds);
                                read_pollers.insert(polled_id, fd);
                            } else {
                                total_ready += 1;
                            }
                        },
                        PolledStatus::BadFd => {
                            *libc::__errno_location() = libc::EBADF;
                            return -1
                        },
                        PolledStatus::NotPollable => libc::FD_CLR(fd, readfds),
                        PolledStatus::ImmediatelyPollable => total_ready += 1,
                    }
                }
            }

            if !writefds.is_null() {
                if libc::FD_ISSET(fd, writefds) {
                    match fd_to_pollout(&mut ctx, fd) {
                        PolledStatus::Pollable(polled_id) => {
                            if !ctx.polled_is_ready(polled_id) {
                                libc::FD_CLR(fd, writefds);
                                write_pollers.insert(polled_id, fd);
                            } else {
                                total_ready += 1;
                            }
                        },
                        PolledStatus::BadFd => {
                            *libc::__errno_location() = libc::EBADF;
                            return -1
                        },
                        PolledStatus::NotPollable => libc::FD_CLR(fd, readfds),
                        PolledStatus::ImmediatelyPollable => total_ready += 1,
                    }
                }
            }
        }

        // TODO: current behavior is to wait indefinitely if there is any timeout
        if total_ready > 0 || (!timeout.is_null() && (*timeout).tv_sec == 0 && (*timeout).tv_nsec == 0) {
            return total_ready
        }

        let poller_id = ctx.new_poller();

        let all_pollers: HashMap<PolledId, RawFd, FxBuildHasher> = read_pollers.clone().into_iter().chain(write_pollers.clone()).collect();
        for (polled_id, _) in all_pollers {
            ctx.register_poller(poller_id, polled_id);
        }

        drop(ctx);
        state::FIZZLE_STATE.yield_thread();
        let mut ctx = state::FIZZLE_STATE.acquire();

        ctx.delete_poller(poller_id);

        for (polled_id, fd) in read_pollers {
            if ctx.polled_is_ready(polled_id) {
                libc::FD_SET(fd, readfds);
                total_ready += 1;
            }
        }

        for (polled_id, fd) in write_pollers {
            if ctx.polled_is_ready(polled_id) {
                libc::FD_SET(fd, writefds);
                total_ready += 1;
            }
        }
    
        total_ready
    }
}

hook_macros::hook! {
    unsafe fn poll(
        fds: *mut libc::pollfd,
        nfds: libc::nfds_t,
        timeout: libc::c_int
    ) -> libc::c_int => fizzle_poll(ctx) {

        drop(ctx);
        if timeout < 0 {
            fizzle_ppoll(fds, nfds, ptr::null(), ptr::null())
        } else {
            let tmo = libc::timespec {
                tv_sec: (timeout / 1000) as i64,
                tv_nsec: ((timeout % 1000) * 1000000) as i64,
            };

            fizzle_ppoll(fds, nfds, ptr::addr_of!(tmo), ptr::null())
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

        // TODO: we just ignore the signal mask for now
        // this may produce undefined behavior
        if !sigmask.is_null() {
            crate::report_strict_failure("fizzle internal error--sigmask unsupported for ppoll")
        }

        let mut total_ready = 0;

        // "Exceptional conditions" never happen in fizzle.

        let mut read_pollers = HashMap::with_hasher(FxBuildHasher::default());
        let mut write_pollers = HashMap::with_hasher(FxBuildHasher::default());

        for i in 0..nfds as usize {
            let pollfd = &mut (*fds.add(i));
            pollfd.revents = 0;
            let mut is_ready = false;
            if (pollfd.events & libc::POLLIN) != 0 {
                match fd_to_pollin(&mut ctx, pollfd.fd) {
                    PolledStatus::Pollable(polled_id) => {
                        if !ctx.polled_is_ready(polled_id) {
                            read_pollers.insert(polled_id, i);
                        } else {
                            pollfd.revents |= libc::POLLIN;
                            is_ready = true;
                        }
                    },
                    PolledStatus::BadFd => {
                        *libc::__errno_location() = libc::EBADF;
                        return -1
                    },
                    PolledStatus::NotPollable => (),
                    PolledStatus::ImmediatelyPollable => {
                        pollfd.revents |= libc::POLLIN;
                        is_ready = true;
                    },
                }
            }

            if (pollfd.events & libc::POLLOUT) != 0 {
                match fd_to_pollout(&mut ctx, pollfd.fd) {
                    PolledStatus::Pollable(polled_id) => {
                        if !ctx.polled_is_ready(polled_id) {
                            write_pollers.insert(polled_id, i);
                        } else {
                            pollfd.revents |= libc::POLLOUT;
                            is_ready = true;
                        }
                    },
                    PolledStatus::BadFd => {
                        *libc::__errno_location() = libc::EBADF;
                        return -1
                    },
                    PolledStatus::NotPollable => (),
                    PolledStatus::ImmediatelyPollable => {
                        pollfd.revents |= libc::POLLOUT;
                        is_ready = true;
                    },
                }
            }

            if is_ready {
                total_ready += 1;
            }
        }

        // TODO: current behavior is to wait indefinitely if there is any timeout
        if total_ready > 0 || (!tmo_p.is_null() && (*tmo_p).tv_sec == 0 && (*tmo_p).tv_nsec == 0) {
            return total_ready
        }

        let poller_id = ctx.new_poller();

        let all_pollers: HashMap<PolledId, usize, FxBuildHasher> = read_pollers.clone().into_iter().chain(write_pollers.clone()).collect();
        for (polled_id, _) in all_pollers {
            ctx.register_poller(poller_id, polled_id);
        }

        drop(ctx);
        state::FIZZLE_STATE.yield_thread();
        let mut ctx = state::FIZZLE_STATE.acquire();

        ctx.delete_poller(poller_id);

        for (polled_id, offset) in read_pollers {
            if ctx.polled_is_ready(polled_id) {
                (*fds.add(offset)).revents |= libc::POLLIN; 
                total_ready += 1;
            }
        }

        for (polled_id, offset) in write_pollers {
            if ctx.polled_is_ready(polled_id) {
                (*fds.add(offset)).revents |= libc::POLLOUT;
                total_ready += 1;
            }
        }
    
        total_ready
    }
}

hook_macros::hook! {
    unsafe fn epoll_create(
        _size: libc::c_int
    ) -> libc::c_int => fizzle_epoll_create(ctx) {
        drop(ctx);
        fizzle_epoll_create1(0)
    }
}

hook_macros::hook! {
    unsafe fn epoll_create1(
        flags: libc::c_int
    ) -> libc::c_int => fizzle_epoll_create1(ctx) {

        let fd = crate::alias_fd_create();
        let epoll_id = ctx.global.epolls.put(EpollInfo { interests: Default::default() });
        ctx.local.fds.insert(DescriptorId::new(fd), FdInfo {
            close_on_exec: (flags & libc::EPOLL_CLOEXEC) != 0,
            nonblocking: false,
            is_passthrough: false,
            resource: FdResource::Epoll(epoll_id),
        });

        fd
    }
}

hook_macros::hook! {
    unsafe fn epoll_ctl(
        epfd: libc::c_int,
        op: libc::c_int,
        fd: libc::c_int,
        event: *mut libc::epoll_event
    ) -> libc::c_int => fizzle_epoll_ctl(ctx) {

        let Some(epfd_info) = ctx.local.fds.get(DescriptorId::new(epfd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Epoll(epoll_id) = epfd_info.resource else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(_) = ctx.local.fds.get(DescriptorId::new(fd)) else {
            log::error!("`epoll_ctl` fd not found (ignoring...)");
            return 0 // TODO: fix fopen rather than this workaround
            //*libc::__errno_location() = libc::EBADF;
            //return -1
        };

        if epfd == fd {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        }
        
        let epoll_info = ctx.global.epolls.get_mut(epoll_id).unwrap();
        match op {
            libc::EPOLL_CTL_ADD => {
                assert!(!event.is_null());
                let descriptor_id = DescriptorId::new(fd);
                if epoll_info.interests.contains_key(&descriptor_id) {
                    *libc::__errno_location() = libc::EEXIST;
                    return -1
                }

                let mut read_status = None;
                let mut write_status = None;

                if ((*event).events & libc::EPOLLIN as u32) != 0 {
                    read_status = Some(fd_to_pollin(&mut ctx, fd));
                }

                if ((*event).events & libc::EPOLLOUT as u32) != 0 {
                    write_status = Some(fd_to_pollout(&mut ctx, fd));
                }

                let direction = match (read_status, write_status) {
                    (None, None) => EpollDirection::None,
                    (Some(status), None) => EpollDirection::Read(status),
                    (None, Some(status)) => EpollDirection::Write(status),
                    (Some(read_status), Some(write_status)) => EpollDirection::Both(read_status, write_status),
                };
                
                let epoll_info = ctx.global.epolls.get_mut(epoll_id).unwrap();
                epoll_info.interests.insert(descriptor_id, EpollInterest {
                    direction,
                    user_data: (*event).u64,
                }).unwrap();

                log::debug!("epfd {} EPOLL_CTL_ADD {} for {}", epfd, fd, match direction {
                    EpollDirection::None => "None",
                    EpollDirection::Read(_) => "EPOLLIN",
                    EpollDirection::Write(_) => "EPOLLOUT",
                    EpollDirection::Both(_, _) => "EPOLLIN | EPOLLOUT",
                });

                0
            }
            libc::EPOLL_CTL_DEL => {
                let Some(_) = epoll_info.interests.remove(&DescriptorId::new(fd)) else {
                    *libc::__errno_location() = libc::ENOENT;
                    return -1
                };

                0
            }
            libc::EPOLL_CTL_MOD => {
                assert!(!event.is_null());

                let mut read_status = None;
                let mut write_status = None;

                if ((*event).events & libc::EPOLLIN as u32) != 0 {
                    read_status = Some(fd_to_pollin(&mut ctx, fd));
                }

                if ((*event).events & libc::EPOLLOUT as u32) != 0 {
                    write_status = Some(fd_to_pollout(&mut ctx, fd));
                }

                let direction = match (read_status, write_status) {
                    (None, None) => EpollDirection::None,
                    (Some(status), None) => EpollDirection::Read(status),
                    (None, Some(status)) => EpollDirection::Write(status),
                    (Some(read_status), Some(write_status)) => EpollDirection::Both(read_status, write_status),
                };

                let epoll_info = ctx.global.epolls.get_mut(epoll_id).unwrap();
                let Some(interest) = epoll_info.interests.get_mut(&DescriptorId::new(fd)) else {
                    *libc::__errno_location() = libc::ENOENT;
                    return -1
                };
                
                interest.direction = direction;
                interest.user_data = (*event).u64;

                0
            }
            _ => {
                crate::report_strict_failure("epoll_ctl invalid argument");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
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
        drop(ctx);
        if timeout < 0 {
            fizzle_epoll_pwait2(epfd, events, maxevents, ptr::null(), ptr::null())
        } else {
            let tmo = libc::timespec {
                tv_sec: (timeout / 1000) as i64,
                tv_nsec: ((timeout % 1000) * 1000000) as i64,
            };

            fizzle_epoll_pwait2(epfd, events, maxevents, ptr::addr_of!(tmo), ptr::null())
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
        drop(ctx);
        if timeout < 0 {
            fizzle_epoll_pwait2(epfd, events, maxevents, ptr::null(), sigmask)
        } else {
            let tmo = libc::timespec {
                tv_sec: (timeout / 1000) as i64,
                tv_nsec: ((timeout % 1000) * 1000000) as i64,
            };

            fizzle_epoll_pwait2(epfd, events, maxevents, ptr::addr_of!(tmo), sigmask)
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

        if !sigmask.is_null() {
            crate::report_strict_failure("sigmask unsupported in epoll_pwait or epoll_pwait2");
        }

        let Some(epfd_info) = ctx.local.fds.get(DescriptorId::new(epfd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Epoll(epoll_id) = epfd_info.resource else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let mut total_ready = 0;

        let poller_id = ctx.new_poller();

        let epoll_info = ctx.global.epolls.get(epoll_id).unwrap();
        for interest in epoll_info.interests.clone().values() {
            match interest.direction {
                EpollDirection::None => (),
                EpollDirection::Read(PolledStatus::NotPollable) => (),
                EpollDirection::Read(PolledStatus::BadFd) => unreachable!(),
                EpollDirection::Read(PolledStatus::ImmediatelyPollable) => {
                    if total_ready < maxevents as usize {
                        let event = &mut (*events.add(total_ready));
                        
                        event.events = libc::EPOLLIN as u32;
                        event.u64 = interest.user_data;
                    }
                    total_ready += 1;
                },
                EpollDirection::Read(PolledStatus::Pollable(polled_id)) => if ctx.polled_is_ready(polled_id) {
                    if total_ready < maxevents as usize {
                        let event = &mut (*events.add(total_ready));
                        event.events = libc::EPOLLIN as u32;
                        event.u64 = interest.user_data;
                    }
                    total_ready += 1;
                } else if total_ready == 0 {
                    ctx.register_poller(poller_id, polled_id);
                }
                EpollDirection::Write(PolledStatus::NotPollable) => (),
                EpollDirection::Write(PolledStatus::BadFd) => unreachable!(),
                EpollDirection::Write(PolledStatus::ImmediatelyPollable) => {
                    if total_ready < maxevents as usize {
                        let event = &mut (*events.add(total_ready));
                        event.events = libc::EPOLLOUT as u32;
                        event.u64 = interest.user_data;
                    }
                    total_ready += 1;
                },
                EpollDirection::Write(PolledStatus::Pollable(polled_id)) => if ctx.polled_is_ready(polled_id) {
                    if total_ready < maxevents as usize {
                        let event = &mut (*events.add(total_ready));
                        event.events = libc::EPOLLOUT as u32;
                        event.u64 = interest.user_data;
                    }
                    total_ready += 1;
                } else if total_ready == 0 {
                    ctx.register_poller(poller_id, polled_id);
                }
                EpollDirection::Both(read_status, write_status) => {
                    let event = &mut (*events.add(total_ready));
                    let mut is_ready = false;
                    event.events = 0;
                    event.u64 = interest.user_data;

                    match read_status {
                        PolledStatus::NotPollable => (),
                        PolledStatus::BadFd => unreachable!(),
                        PolledStatus::ImmediatelyPollable => {
                            if total_ready < maxevents as usize {
                                event.events |= libc::EPOLLIN as u32;
                            }
                            is_ready = true;
                        }
                        PolledStatus::Pollable(polled_id) => if ctx.polled_is_ready(polled_id) {
                            if total_ready < maxevents as usize {
                                event.events |= libc::EPOLLIN as u32;
                            }
                            is_ready = true;
                        } else if total_ready == 0 {
                            ctx.register_poller(poller_id, polled_id);
                        }
                    }

                    match write_status {
                        PolledStatus::NotPollable => (),
                        PolledStatus::BadFd => unreachable!(),
                        PolledStatus::ImmediatelyPollable => {
                            if total_ready < maxevents as usize {
                                event.events |= libc::EPOLLOUT as u32;
                                event.u64 = interest.user_data;
                            }
                            is_ready = true;
                        },
                        PolledStatus::Pollable(polled_id) => if ctx.polled_is_ready(polled_id) {
                            if total_ready < maxevents as usize {
                                event.events |= libc::EPOLLOUT as u32;
                                event.u64 = interest.user_data;
                            }
                            is_ready = true;
                        } else if total_ready == 0 {
                            ctx.register_poller(poller_id, polled_id);
                        }
                    }

                    if is_ready {
                        total_ready += 1;
                    }
                },
            }
        }

        if total_ready != 0 || (!timeout.is_null() && (*timeout).tv_sec == 0 && (*timeout).tv_nsec == 0) {
            ctx.delete_poller(poller_id);
            return total_ready as libc::c_int
        }

        drop(ctx);
        state::FIZZLE_STATE.yield_thread();
        let mut ctx = state::FIZZLE_STATE.acquire();

        // It's unfortunate that we have to delete the poller each time.
        // This is a worst-case O(m*n) operation, though most times m=1 so it isn't unacceptable performance per se.
        // The issue is that multiple threads could `epoll_wait` on the same epoll fd, which would lead to weird
        // behavior if we kept the poller saved between calls.
        ctx.delete_poller(poller_id);

        let epoll_info = ctx.global.epolls.get(epoll_id).unwrap();
        for interest in epoll_info.interests.clone().values() {
            match interest.direction {
                EpollDirection::Read(PolledStatus::Pollable(polled_id)) => if ctx.polled_is_ready(polled_id) {
                    if total_ready < maxevents as usize {
                        let event = &mut (*events.add(total_ready));
                        event.events = libc::EPOLLIN as u32;
                        event.u64 = interest.user_data;
                    }
                    total_ready += 1;
                }
                EpollDirection::Read(_) => (),
                EpollDirection::Write(PolledStatus::Pollable(polled_id)) => if ctx.polled_is_ready(polled_id) {
                    if total_ready < maxevents as usize {
                        let event = &mut (*events.add(total_ready));
                        event.events = libc::EPOLLOUT as u32;
                        event.u64 = interest.user_data;
                    }
                    total_ready += 1;
                }
                EpollDirection::Write(_) => (),
                EpollDirection::Both(read_status, write_status) => {
                    let event = &mut (*events.add(total_ready));
                    let mut is_ready = false;
                    event.events = 0;
                    event.u64 = interest.user_data;

                    if let PolledStatus::Pollable(polled_id) = read_status {
                        if ctx.polled_is_ready(polled_id) {
                            if total_ready < maxevents as usize {
                                event.events |= libc::EPOLLIN as u32;
                            }
                            is_ready = true;
                        }
                    }

                    if let PolledStatus::Pollable(polled_id) = write_status {
                        if ctx.polled_is_ready(polled_id) {
                            if total_ready < maxevents as usize {
                                event.events |= libc::EPOLLOUT as u32;
                            }
                            is_ready = true;
                        }
                    }

                    if is_ready {
                        total_ready += 1;
                    }
                },
                EpollDirection::None => (),
            }
        }
    
        total_ready as libc::c_int
    }
}
