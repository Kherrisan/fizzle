//! Hooks for general functions that can be applied to any file descriptor.
//!

use std::mem;

use crate::backend::ConnectedBackend;
use crate::handlers::descriptor::{DescriptorId, DescriptorInfo, FdResource};
use crate::handlers::socket::SocketState;
use crate::hook_macros;

hook_macros::hook! {
    unsafe fn close(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_close(ctx) {
        let mut state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);
        let ref_count = state.local.fds.ref_count(&descriptor_id);

        if ref_count == 1 {
            log::debug!("close({}) -> 0 (last fd closed for resource)", fd);
            match state.local.fds.get(&descriptor_id) {
                Some(DescriptorInfo { resource: FdResource::Epoll(_), .. }) => crate::alias_fd_destroy(fd),
                Some(DescriptorInfo { resource: FdResource::EventFd(_), .. }) => crate::alias_fd_destroy(fd),
                Some(DescriptorInfo { resource: FdResource::Directory(_), .. }) => crate::alias_fd_destroy(fd),
                Some(DescriptorInfo { resource: FdResource::File(_), .. }) => crate::alias_fd_destroy(fd),
                Some(DescriptorInfo { resource: FdResource::Socket(socket_id), .. }) => {
                    assert_eq!(hook_macros::real!(close)(fd), 0, "close() returned nonzero despite valid socket context");
                
                    let socket_id = socket_id.clone();
                    let mut ref_count = state.global.sockets.ref_count(&socket_id);
                    match state.global.sockets.get(&socket_id).unwrap() {
                        SocketState::Connectionless(sock_info) => if ref_count <= 3 { // 2 refs we're currently holding + 1 ref in socket_locations
                            let addr = sock_info.local_addr.clone();
                            state.global.socket_locations.remove(&addr).unwrap();
                        }
                        SocketState::Unassociated(sock_info) => {
                            if ref_count <= 3 {
                                if let Some(addr) = sock_info.local_addr.clone() {
                                    state.global.socket_locations.remove(&addr).unwrap();                                   
                                }
                            }
                        }
                        SocketState::Server(server_info) => {
                            if ref_count <= 3 {
                                // TODO: need to handle `connecting` sockets here so that they get rejected correctly
                                let addr = server_info.local_addr.clone();
                                state.global.socket_locations.remove(&addr).unwrap();
                            }
                        }
                        SocketState::Connecting(connecting_info) => {
                            if ref_count <= 3 {
                                let addr = connecting_info.local_addr.clone();
                                state.global.socket_locations.remove(&addr).unwrap();
                            }
                        }
                        SocketState::Connected(connection_info) => {
                            // TODO: local address never bound?? Need to move address location...
                            if let ConnectedBackend::Peered(peer_info) = &connection_info.backend {
                                let peer_id = peer_info.peer.clone().unwrap();
                                let SocketState::Connected(peer_info) = state.global.sockets.get_mut(&peer_id).unwrap() else {
                                    unreachable!()
                                };

                                if let ConnectedBackend::Peered(p) = &mut peer_info.backend {
                                    p.peer = None;
                                    ref_count -= 1;
                                };

                                if ref_count <= 3 {
                                    peer_info.peer_closed = true;
                                    let addr = peer_info.rem_addr.clone();

                                    // TODO: what about accept()ed sockets?
                                    state.global.socket_locations.remove(&addr).unwrap();
                                }
                            }
                        }
                        SocketState::PendingConnection(_) => unreachable!(),
                    }
                },
                Some(DescriptorInfo { resource: FdResource::MessageQueue(_), .. }) => crate::alias_fd_destroy(fd),
                Some(DescriptorInfo { resource: FdResource::Pipe(pipe_id), .. }) => {
                    crate::alias_fd_destroy(fd);
                    if let Some(peer_id) = state.global.pipes.get(&pipe_id).unwrap().peer.clone() {
                        state.global.pipes.get_mut(&peer_id).unwrap().peer = None;                       
                    }
                }
                // TODO: mark stdin as closed after this...
                Some(DescriptorInfo { resource: FdResource::Stdin, .. }) => (), // We keep stdin for fuzzing input...
                Some(DescriptorInfo { resource: FdResource::Stdout, .. }) => (),
                Some(DescriptorInfo { resource: FdResource::Stderr, .. }) => (), // ... and stderr for reporting Fizzle errors.
                None => unreachable!(),
            }
        } else if ref_count == 0 {
            *libc::__errno_location() = libc::EBADFD;
            return -1
        }

        assert_eq!(state.local.fds.downref(&descriptor_id), ref_count - 1);

        0
    }
}

// GNU libc unconditionally pulls a void* from va_args, so we should (hypothetically?) be okay doing this.
hook_macros::hook! {
    unsafe fn fcntl(
        fd: libc::c_int,
        cmd: libc::c_int,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_fcntl(ctx) {
        let mut state = ctx.acquire();

        match state.local.fds.get_mut(&DescriptorId::from_raw_fd(fd)) {
            Some(fd_info) if fd_info.is_passthrough => {
                let dupfd = hook_macros::real!(fcntl)(fd, cmd, arg);
                if dupfd >= 0 && (cmd == libc::F_DUPFD || cmd == libc::F_DUPFD_CLOEXEC) {
                    let nonblocking = fd_info.nonblocking;
                    let close_on_exec = cmd == libc::F_DUPFD_CLOEXEC;
                    let resource = fd_info.resource.clone();
                    state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(dupfd), DescriptorInfo {
                        close_on_exec,
                        nonblocking,
                        is_passthrough: true,
                        resource,
                    }).unwrap();
                }

                dupfd
            }
            Some(fd_info) => {
                match cmd {
                    libc::F_GETFL => return if fd_info.nonblocking { libc::O_NONBLOCK } else { 0 }, // TODO: support remaining flags
                    libc::F_SETFL => {
                        fd_info.nonblocking = ((arg as usize) & libc::O_NONBLOCK as usize) > 0;
                        return 0
                    }
                    libc::F_GETFD => return if fd_info.close_on_exec { libc::O_CLOEXEC } else { 0 },
                    libc::F_SETFD => {
                        fd_info.close_on_exec = ((arg as usize) & libc::O_CLOEXEC as usize) > 0;
                        return 0;
                    }
                    libc::F_DUPFD | libc::F_DUPFD_CLOEXEC => {
                        let nonblocking = fd_info.nonblocking;
                        let resource = fd_info.resource.clone();

                        let dupfd = crate::alias_fd_create();
                        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(dupfd), DescriptorInfo {
                            close_on_exec: cmd == libc::F_DUPFD_CLOEXEC,
                            nonblocking,
                            is_passthrough: false,
                            resource,
                        }).unwrap();

                        return dupfd
                    }
                    libc::F_SETLK | libc::F_SETLKW | libc::F_GETLK => {
                        crate::report_strict_failure("unimplemented fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_GETOWN | libc::F_SETOWN => {
                        crate::report_strict_failure("unimplemented OWN fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_GETLEASE | libc::F_SETLEASE => {
                        crate::report_strict_failure("unimplemented LEASE fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_NOTIFY => {
                        crate::report_strict_failure("unimplemented fcntl command F_NOTIFY");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_SETPIPE_SZ => {
                        crate::report_strict_failure("unimplemented fcntl command F_SETPIPE_SZ");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    libc::F_ADD_SEALS | libc::F_GET_SEALS => {
                        crate::report_strict_failure("unimplemented fcntl command(s) SEALS");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    }
                    // libc::F_GET_RW_HINT | libc::F_SET_RW_HINT | libc::F_GET_FILE_RW_HINT | libc::F_SET_FILE_RW_HINT => unimplemented by libc
                    _ => {
                        crate::report_strict_failure("unrecognized fcntl command");
                        *libc::__errno_location() = libc::EINVAL;
                        return -1
                    },
                }
            },
            None => {
                *libc::__errno_location() = libc::EBADF;
                return -1
            },
        }


    }
}

// GNU libc unconditionally pulls a void* from va_args, so we should (hypothetically?) be okay doing this.
hook_macros::hook! {
    unsafe fn ioctl(
        fd: libc::c_int,
        request: libc::c_int,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_ioctl(_ctx) {
        log::info!("ioctl({}, {}, {})", fd, request, arg as usize);

        panic!("`ioctl` unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn dup(
        oldfd: libc::c_int
    ) -> libc::c_int => fizzle_dup(ctx) {
        let mut state = ctx.acquire();

        match state.local.fds.get_mut(&DescriptorId::from_raw_fd(oldfd)) {
            Some(fd_info) => {
                let new_fd_info = fd_info.clone();
                let new_fd = crate::alias_fd_create();
                state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(new_fd), new_fd_info).unwrap();
                new_fd
            }
            None => {
                log::warn!("dup() called on unrecognized file descriptor {}", oldfd);
                *libc::__errno_location() = libc::EBADF;
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn dup2(
        oldfd: libc::c_int,
        newfd: libc::c_int
    ) -> libc::c_int => fizzle_dup2(ctx) {
        if oldfd == newfd {
            return newfd
        }

        let mut state = ctx.acquire();

        match state.local.fds.get_mut(&DescriptorId::from_raw_fd(oldfd)) {
            Some(fd_info) => {
                let mut new_fd_info = fd_info.clone();

                match state.local.fds.get_mut(&DescriptorId::from_raw_fd(newfd)) {
                    Some(old_info) => {
                        mem::swap(old_info, &mut new_fd_info);
                        drop(new_fd_info);
                    }
                    None => {
                        libc::dup2(oldfd, newfd);
                        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(newfd), new_fd_info).unwrap();
                    }
                }

                newfd
            }
            None => {
                log::warn!("dup() called on unrecognized file descriptor {}", oldfd);
                *libc::__errno_location() = libc::EBADF;
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn dup3(
        oldfd: libc::c_int,
        newfd: libc::c_int,
        _flags: libc::c_int
    ) -> libc::c_int => fizzle_dup3(ctx) {

        // TODO: handle flags; handle lack of flags in other dups

        if oldfd == newfd {
            return newfd
        }

        let mut state = ctx.acquire();

        match state.local.fds.get_mut(&DescriptorId::from_raw_fd(oldfd)) {
            Some(fd_info) => {
                let mut new_fd_info = fd_info.clone();

                match state.local.fds.get_mut(&DescriptorId::from_raw_fd(newfd)) {
                    Some(old_info) => {
                        mem::swap(old_info, &mut new_fd_info);
                        drop(new_fd_info);
                    }
                    None => {
                        libc::dup2(oldfd, newfd);
                        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(newfd), new_fd_info).unwrap();
                    }
                }

                newfd
            }
            None => {
                log::warn!("dup() called on unrecognized file descriptor {}", oldfd);
                *libc::__errno_location() = libc::EBADF;
                -1
            }
        }
    }
}
