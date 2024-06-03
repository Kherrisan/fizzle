use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::{cmp, mem, ptr, slice};

use fizzle_common::io::TransportAddress;
use fizzle_common::storage::RingBuffer;

use crate::hook_macros;
use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::state::backend::{ConnectedBackend, ConnectionlessBackend, FileBackend, IoBackend, RegularConnected, StdioBackend};
use crate::state::identifiers::{BufferId, PolledId, SocketId};
use crate::state::{ConnectedSocket, ConnectionlessSocket, FizzleContext, SocketLocationInfo, SocketState};
use crate::state::fd::FdResource;
use crate::state::identifiers::DescriptorId;

fn read_from_buffer(ctx: &mut FizzleContext, data: &mut [u8], buffer_id: BufferId, read_polled: PolledId, write_polled: Option<PolledId>, is_nonblocking: bool) -> libc::ssize_t {
    if !ctx.global().polled_events.get(read_polled).unwrap().event_raised {
        if is_nonblocking {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            return -1
        } else {
            ctx.poll_until_ready(read_polled);
        }
    }

    let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
    let read_len = buf.read(data);
    if buf.is_empty() {
        ctx.lower_polled(read_polled);
    }
    if let Some(write_polled) = write_polled {
        ctx.raise_polled(write_polled)
    }

    return read_len as isize
}

fn write_to_buffer(ctx: &mut FizzleContext, data: &[u8], buffer_id: BufferId, write_polled: PolledId, read_polled: Option<PolledId>, is_nonblocking: bool) -> libc::ssize_t {
    if !ctx.global().polled_events.get(write_polled).unwrap().event_raised {
        if is_nonblocking {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            return -1
        } else {
            ctx.poll_until_ready(write_polled);
        }
    }

    let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
    let written = buf.write(data);
    if buf.is_full() {
        ctx.lower_polled(write_polled);
    }
    if let Some(read_polled) = read_polled {
        ctx.raise_polled(read_polled)
    }

    return written as isize
}

hook_macros::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_write(ctx) {

        let Some(fd_info) = ctx.local().fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking;
        let data = slice::from_raw_parts(buf as *const u8, len);

        match fd_info.resource {
            FdResource::Epoll(_) | FdResource::Directory(_) => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            },
            FdResource::File(file_id) => match ctx.global().files.get(file_id).unwrap() {
                FileBackend::Passthrough => hook_macros::real!(write)(fd, buf, len),
                FileBackend::Regular(_) => unreachable!(),
                FileBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let write_polled = feedback.write_polled;
                    let read_polled = feedback.read_polled;

                    write_to_buffer(&mut ctx, data, buffer_id, write_polled, Some(read_polled), is_nonblocking)
                }
                FileBackend::Plugin(plugin) => {
                    let buffer_id = plugin.write_buf;
                    let write_polled = plugin.write_polled;

                    write_to_buffer(&mut ctx, data, buffer_id, write_polled, None, is_nonblocking)
                },
                FileBackend::Sink => len as libc::ssize_t,
                FileBackend::NullSink => len as libc::ssize_t,
                FileBackend::Fuzz(_) => len as libc::ssize_t,
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => {
                let Some(peer_id) = ctx.global().pipes.get(pipe_id).unwrap().peer else {
                    *libc::__errno_location() = libc::EPIPE;
                    return -1
                };

                let peer_info = ctx.global().pipes.get(peer_id).unwrap();
                let buffer_id = peer_info.read_buf;
                let write_polled = peer_info.write_polled;
                let read_polled = peer_info.read_polled;
                
                if !ctx.polled_is_ready(write_polled) {
                    if is_nonblocking {
                        unsafe { *libc::__errno_location() = libc::EAGAIN };
                        return -1
                    } else {
                        ctx.poll_until_ready(write_polled);
                    }
                }

                // We need to verify that this connection has not shut down before writing to the same buffer_id
                if ctx.global().pipes.get(pipe_id).unwrap().peer.is_none() {
                    unsafe { *libc::__errno_location() = libc::EPIPE };
                    return -1
                };

                let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
                let written = buf.write(data);
                if buf.is_full() {
                    ctx.lower_polled(write_polled);
                }
                ctx.raise_polled(read_polled);

                return written as isize
            },
            FdResource::Stdin => 0,
            FdResource::Stdout => match ctx.global().stdio {
                StdioBackend::Passthrough => unreachable!(),
                StdioBackend::Regular(_) => unreachable!(),
                StdioBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let write_polled = feedback.write_polled;
                    let read_polled = feedback.read_polled;

                    write_to_buffer(&mut ctx, data, buffer_id, write_polled, Some(read_polled), is_nonblocking)
                },
                StdioBackend::Plugin(plugin) => {
                    let buffer_id = plugin.write_buf;
                    let write_polled = plugin.write_polled;

                    write_to_buffer(&mut ctx, data, buffer_id, write_polled, None, is_nonblocking)
                },
                StdioBackend::Sink => len as libc::ssize_t,
                StdioBackend::NullSink => len as libc::ssize_t,
                StdioBackend::Fuzz(_) => len as libc::ssize_t,
            },
            FdResource::Stderr => len as libc::ssize_t, // Transparently consume `stderr` output
            FdResource::Socket(socket_id) => match ctx.global().sockets.get(socket_id).unwrap() {
                SocketState::Connected(_) => send_connected_socket(&mut ctx, data, socket_id, is_nonblocking),
                SocketState::Connectionless(_) => send_connectionless_socket(&mut ctx, data, socket_id, is_nonblocking, None),
                _ => {
                    *libc::__errno_location() = libc::ENOTCONN;
                    return -1
                }
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn send(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_send(ctx) {

        if (flags & libc::MSG_FASTOPEN) != 0 {
            crate::report_strict_failure("fizzle does not currently implement TCP Fast Open")
        }

        let Some(fd_info) = ctx.local().fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);
        let data = slice::from_raw_parts(buf as *const u8, len);

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global().sockets.get(socket_id).unwrap() {
            SocketState::Connected(_) => send_connected_socket(&mut ctx, data, socket_id, is_nonblocking),
            SocketState::Connectionless(_) => send_connectionless_socket(&mut ctx, data, socket_id, is_nonblocking, None),
            _ => {
                *libc::__errno_location() = libc::ENOTCONN;
                return -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sendto(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t,
        flags: libc::c_int,
        dest_addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::ssize_t => fizzle_sendto(ctx) {

        if (flags & libc::MSG_FASTOPEN) != 0 {
            crate::report_strict_failure("fizzle does not currently implement TCP Fast Open")
        }

        let Some(fd_info) = ctx.local().fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        let data = slice::from_raw_parts(buf as *const u8, len);

        match ctx.global().sockets.get(socket_id).unwrap() {
            SocketState::Connectionless(_) => {
                let addr = if dest_addr.is_null() {
                    None
                } else {
                    match crate::decode_inet_address(dest_addr, addrlen) {
                        Ok(addr) => Some(addr),
                        Err(_) => {
                            *libc::__errno_location() = libc::EINVAL;
                            return -1
                        }
                    }
                };

                send_connectionless_socket(&mut ctx, data, socket_id, is_nonblocking, addr)
            }
            SocketState::Connected(_) => {
                if addrlen != 0 || !dest_addr.is_null() {
                    *libc::__errno_location() = libc::EISCONN;
                    return -1
                }
                send_connected_socket(&mut ctx, data, socket_id, is_nonblocking)
            }
            _ => {
                *libc::__errno_location() = libc::ENOTCONN;
                return -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sendmsg(
        fd: libc::c_int,
        msg: *const libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_sendmsg(_ctx) {

        if (flags & libc::MSG_FASTOPEN) != 0 {
            crate::report_strict_failure("fizzle does not currently implement TCP Fast Open")
        }

        crate::report_strict_failure("`sendmsg` unimplemented");
        hook_macros::real!(sendmsg)(fd, msg, flags)
    }
}

hook_macros::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_read(ctx) {

        let Some(fd_info) = ctx.local().fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking;
        let data = slice::from_raw_parts_mut(buf as *mut u8, len);

        match fd_info.resource {
            FdResource::Epoll(_) | FdResource::Directory(_) => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            },
            FdResource::File(file_id) => match ctx.global().files.get(file_id).unwrap() {
                crate::state::backend::IoBackend::Passthrough => hook_macros::real!(read)(fd, buf, len),
                crate::state::backend::IoBackend::Regular(_) => unreachable!(),
                crate::state::backend::IoBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let read_polled = feedback.read_polled;
                    let write_polled = feedback.write_polled;

                    read_from_buffer(&mut ctx, data, buffer_id, read_polled, Some(write_polled), is_nonblocking)
                },
                crate::state::backend::IoBackend::Plugin(plugin) => {
                    let buffer_id = plugin.read_buf;
                    let read_polled = plugin.read_polled;

                    read_from_buffer(&mut ctx, data, buffer_id, read_polled, None, is_nonblocking)                   
                },
                crate::state::backend::IoBackend::Sink => 0 as libc::ssize_t,
                crate::state::backend::IoBackend::NullSink => {
                    for b in data.iter_mut() {
                        *b = 0;
                    }
                    data.len() as libc::ssize_t
                },
                crate::state::backend::IoBackend::Fuzz(_) => todo!(),
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => {
                let pipe_info = ctx.global().pipes.get(pipe_id).unwrap();
                let peer_is_closed = pipe_info.peer.is_none();

                let buffer_id = pipe_info.read_buf;
                let write_polled = pipe_info.write_polled;
                let read_polled = pipe_info.read_polled;
                
                if !ctx.polled_is_ready(write_polled) {
                    if peer_is_closed {
                        return 0
                    } else if is_nonblocking {
                        unsafe { *libc::__errno_location() = libc::EAGAIN };
                        return -1
                    } else {
                        ctx.poll_until_ready(write_polled);
                    }
                }

                if ctx.global().pipes.get(pipe_id).unwrap().peer.is_none() {
                    return 0
                }

                let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
                let amount_written = buf.read(data);
                
                if buf.is_empty() {
                    ctx.lower_polled(read_polled);
                }
                ctx.raise_polled(write_polled);

                return amount_written as isize
            },
            FdResource::Stdin => match ctx.global().stdio {
                StdioBackend::Passthrough => unreachable!(),
                StdioBackend::Regular(_) => unreachable!(),
                StdioBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let read_polled = feedback.read_polled;
                    let write_polled = feedback.write_polled;

                    read_from_buffer(&mut ctx, data, buffer_id, read_polled, Some(write_polled), is_nonblocking)
                },
                StdioBackend::Plugin(plugin) => {
                    let buffer_id = plugin.write_buf;
                    let read_polled = plugin.read_polled;

                    read_from_buffer(&mut ctx, data, buffer_id, read_polled, None, is_nonblocking)
                },
                StdioBackend::Sink => 0,
                StdioBackend::NullSink => {
                    for b in data.iter_mut() {
                        *b = 0;
                    }
                    data.len() as libc::ssize_t
                },
                StdioBackend::Fuzz(_) => todo!(),
            },
            FdResource::Stdout => 0,
            FdResource::Stderr => 0,
            FdResource::Socket(socket_id) => match ctx.global().sockets.get(socket_id).unwrap() {
                SocketState::Connected(_) => send_connected_socket(&mut ctx, data, socket_id, is_nonblocking),
                SocketState::Connectionless(_) => send_connectionless_socket(&mut ctx, data, socket_id, is_nonblocking, None),
                _ => {
                    *libc::__errno_location() = libc::ENOTCONN;
                    return -1
                }
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn recv(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recv(ctx) {

        crate::report_strict_failure("`recv` unimplemented");
        drop(ctx);
        fizzle_recvfrom(fd, buf, len, flags, ptr::null_mut(), ptr::null_mut())
    }
}

hook_macros::hook! {
    unsafe fn recvfrom(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t,
        flags: libc::c_int,
        src_addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::ssize_t => fizzle_recvfrom(ctx) {

        let Some(fd_info) = ctx.local().fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);
        let data = slice::from_raw_parts_mut(buf as *mut u8, len);

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global().sockets.get(socket_id).unwrap() {
            SocketState::Connected(conn_info) => {
                crate::encode_inet_address(src_addr, conn_info.rem_addr.address()); // TODO: buffer overflow if addrlen is too short...
                recv_connected_socket(&mut ctx, data, socket_id, is_nonblocking)
            },
            SocketState::Connectionless(_) => {
                recv_connectionless_socket(&mut ctx, data, socket_id, is_nonblocking, src_addr, addrlen)

            },
            _ => {
                *libc::__errno_location() = libc::ENOTCONN;
                return -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn recvmsg(
        fd: libc::c_int,
        msg: *mut libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recvmsg(_ctx) {

        crate::report_strict_failure("`recvmsg` unimplemented");
        hook_macros::real!(recvmsg)(fd, msg, flags)
    }
}

hook_macros::hook! {
    unsafe fn recvmmsg(
        fd: libc::c_int,
        msgvec: *mut libc::msghdr,
        vlen: libc::c_uint,
        flags: libc::c_int,
        timeout: *mut libc::timespec
    ) -> libc::ssize_t => fizzle_recvmmsg(_ctx) {

        crate::report_strict_failure("`recvmsg` unimplemented");
        hook_macros::real!(recvmmsg)(fd, msgvec, vlen, flags, timeout)
    }
}

fn write_datagram<const N: usize>(
    send_buf: &mut RingBuffer<N>,
    data: &[u8],
    addr: &SocketAddr,
) -> libc::ssize_t {
    let mut sockaddr: MaybeUninit<libc::sockaddr_storage> = MaybeUninit::uninit();
    unsafe { crate::encode_inet_address(sockaddr.as_mut_ptr() as *mut libc::sockaddr, addr) };
    let addrlen = match unsafe { sockaddr.assume_init().ss_family } as i32 {
        libc::AF_INET => mem::size_of::<libc::sockaddr_in>(),
        libc::AF_INET6 => mem::size_of::<libc::sockaddr_in6>(),
        _ => panic!("internal fizzle error--unrecognized socket address written to datagram"),
    };

    let sockaddr_bytes = unsafe { slice::from_raw_parts(sockaddr.as_ptr() as *const u8, addrlen) };

    // There is sufficient space--send the datagram
    let sockaddr_len = sockaddr_bytes.len() as u8;
    assert!(send_buf.write(slice::from_ref(&sockaddr_len)) == 1);

    let written = send_buf.write(sockaddr_bytes);
    assert!(written == sockaddr_len as usize);

    let data_len = data.len() as u16;
    assert!(send_buf.write(&data_len.to_be_bytes()) == 2);

    let written = send_buf.write(data);
    assert!(written == data.len());

    data.len() as libc::ssize_t
}

fn send_connected_socket(
    ctx: &mut FizzleContext,
    data: &[u8],
    socket_id: SocketId,
    is_nonblocking: bool,
) -> libc::ssize_t {
    let SocketState::Connected(sock_info) = ctx.global().sockets.get(socket_id).unwrap() else {
        panic!("internal error")
    };

    let ConnectedBackend::Regular(regular) = sock_info.backend else {
        unreachable!()
    };

    let Some(peer) = regular.peer else {
        return 0; // No more information to write to the connected socket
    };

    let Some(SocketState::Connected(peer_info)) = ctx.global().sockets.get(peer) else {
        unreachable!()
    };

    // TODO: potentially make this more DRY
    match peer_info.backend {
        ConnectedBackend::Passthrough => unimplemented!(),
        ConnectedBackend::Regular(regular_peer) => {
            let buffer_id = regular_peer.recv_buf;
            let write_polled = regular_peer.write_polled;
            let read_polled = regular_peer.read_polled;

            if !ctx.polled_is_ready(write_polled) {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    ctx.poll_until_ready(write_polled);
                }
            }

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket { backend: IoBackend::Regular(RegularConnected { peer: Some(_), .. }), .. })) = ctx.global().sockets.get(socket_id) else {
                return 0
            };

            let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
            let written = buf.write(data);
            if buf.is_full() {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return written as isize
        },
        ConnectedBackend::Feedback(feedback) => {
            let buffer_id = feedback.buf;
            let write_polled = feedback.write_polled;
            let read_polled = feedback.read_polled;

            if !ctx.polled_is_ready(write_polled) {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    ctx.poll_until_ready(write_polled);
                }
            }

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket { backend: IoBackend::Regular(RegularConnected { peer: Some(_), .. }), .. })) = ctx.global().sockets.get(socket_id) else {
                return 0
            };

            let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
            let written = buf.write(data);
            if buf.is_full() {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return written as isize
        },
        ConnectedBackend::Plugin(plugin) => {
            let buffer_id = plugin.write_buf;
            let write_polled = plugin.write_polled;
            let read_polled = plugin.read_polled;

            if !ctx.polled_is_ready(write_polled) {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    ctx.poll_until_ready(write_polled);
                }
            }

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket { backend: IoBackend::Regular(RegularConnected { peer: Some(_), .. }), .. })) = ctx.global().sockets.get(socket_id) else {
                return 0
            };

            let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
            let written = buf.write(data);
            if buf.is_full() {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return written as isize
        },
        ConnectedBackend::Sink => data.len() as libc::ssize_t,
        ConnectedBackend::NullSink => data.len() as libc::ssize_t,
        ConnectedBackend::Fuzz(_) => data.len() as libc::ssize_t,
    }
}

const MAX_DATAGRAM: usize = 65507;
const MAX_INTERNAL_DATAGRAM: usize = MAX_DATAGRAM + mem::size_of::<libc::sockaddr_storage>() + 3;

fn send_connectionless_socket(
    ctx: &mut FizzleContext,
    data: &[u8],
    socket_id: SocketId,
    is_nonblocking: bool,
    addr: Option<SocketAddr>,
) -> libc::ssize_t {

    let SocketState::Connectionless(sock_info) = ctx.global().sockets.get(socket_id).unwrap()
    else {
        unreachable!()
    };

    if data.len() > MAX_DATAGRAM {
        unsafe { *libc::__errno_location() = libc::EMSGSIZE };
        return -1;
    }

    let Some(rem_addr) = addr.or_else(|| sock_info.rem_addr) else {
        unsafe { *libc::__errno_location() = libc::ENOTCONN };
        return -1;
    };

    let Some(SocketLocationInfo {
        bound_socket: Some(peer_sock_id),
        ..
    }) = ctx
        .global()
        .socket_locations
        .get(&TransportAddress::Udp(rem_addr))
    else {
        unsafe { *libc::__errno_location() = libc::ECONNRESET }; // No socket was listening at the endpoint
        return -1; // TODO: should we just return data.len() here instead?
    };
    let peer_sock_id = *peer_sock_id;

    let SocketState::Connectionless(peer_info) = ctx.global().sockets.get(peer_sock_id).unwrap()
    else {
        unreachable!()
    };

    match peer_info.backend {
        ConnectionlessBackend::Passthrough => unimplemented!(),
        ConnectionlessBackend::Regular(regular_peer) => {
            let write_polled = regular_peer.write_polled;

            if !ctx.polled_is_ready(write_polled) {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    ctx.poll_until_ready(write_polled);
                }
            }

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketLocationInfo {
                bound_socket: Some(peer_sock_id),
                ..
            }) = ctx
                .global()
                .socket_locations
                .get(&TransportAddress::Udp(rem_addr))
            else {
                return data.len() as libc::ssize_t // Drop packet
            };

            let peer_sock_id = *peer_sock_id;


            let SocketState::Connectionless(ConnectionlessSocket { backend: IoBackend::Regular(regular_peer), .. }) = ctx.global().sockets.get(peer_sock_id).unwrap() else {
                return data.len() as libc::ssize_t // Drop packet
            };

            let buffer_id = regular_peer.recv_buf;
            let write_polled = regular_peer.write_polled;
            let read_polled = regular_peer.read_polled;

            // Re-doing all this accounts for a nasty (though unlikely) TOCTOU bug that could show up if the
            // destination UDP server disconnects and another takes it place while this thread is polling.
            if !ctx.polled_is_ready(write_polled) {
                return data.len() as libc::ssize_t // Drop packet
            }

            let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &rem_addr);
            
            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return amount_written as isize
        },
        ConnectionlessBackend::Feedback(feedback) => {
            let buffer_id = feedback.buf;
            let write_polled = feedback.write_polled;
            let read_polled = feedback.read_polled;

            if !ctx.polled_is_ready(write_polled) {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    ctx.poll_until_ready(write_polled);
                }
            }

            // We don't need to verify that this connection has not shut down, as it's a Feedback endpoint

            let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &rem_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return amount_written as isize
        },
        ConnectionlessBackend::Plugin(plugin) => {
            let buffer_id = plugin.write_buf;
            let write_polled = plugin.write_polled;

            if !ctx.polled_is_ready(write_polled) {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    ctx.poll_until_ready(write_polled);
                }
            }

            // We don't need to verify that this connection has not shut down, as it's a Plugin endpoint

            let buf = ctx.global().buffers.get_mut(buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &rem_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(write_polled);
            }

            return amount_written as isize
        },
        ConnectionlessBackend::Sink => data.len() as libc::ssize_t,
        ConnectionlessBackend::NullSink => data.len() as libc::ssize_t,
        ConnectionlessBackend::Fuzz(_) => data.len() as libc::ssize_t,
    }
}

fn read_datagram<const N: usize>(
    recv_buf: &mut RingBuffer<N>,
    data: &mut [u8],
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> libc::ssize_t {
    let mut stored_addrlen = 0u8;
    assert!(recv_buf.read(slice::from_mut(&mut stored_addrlen)) == 1);

    let mut addr_buf = [0u8; 128];
    let amount_read = recv_buf.read(&mut addr_buf[..stored_addrlen as usize]);
    assert!(amount_read == stored_addrlen as usize);

    if !addr.is_null() && !addrlen.is_null() {
        unsafe {
            let write_addrlen = cmp::min(stored_addrlen as usize, *addrlen as usize);
            libc::memcpy(
                addr as *mut libc::c_void,
                addr_buf.as_ptr() as *const libc::c_void,
                write_addrlen,
            );
            *addrlen = stored_addrlen as libc::socklen_t;
        }
    }

    let mut pktlen_buf = [0u8; 2];
    assert!(recv_buf.read(&mut pktlen_buf) == 2);

    let packet_len = u16::from_be_bytes(pktlen_buf) as usize;
    let read_len = cmp::min(packet_len, data.len());

    let amount_read = recv_buf.read(&mut data[..read_len]);
    assert!(amount_read == read_len);

    packet_len as libc::ssize_t
}

fn recv_connected_socket(
    ctx: &mut FizzleContext,
    data: &mut [u8],
    socket_id: SocketId,
    is_nonblocking: bool,
) -> libc::ssize_t {

    let SocketState::Connected(sock_info) = ctx.global().sockets.get(socket_id).unwrap() else {
        panic!("internal error")
    };

    let ConnectedBackend::Regular(regular) = sock_info.backend else {
        unreachable!()
    };

    let buf_id = regular.recv_buf;
    let write_polled = regular.write_polled;
    let read_polled = regular.read_polled;

    // First, check to see if we can just immediately read despite teh peer being closed
    if ctx.polled_is_ready(read_polled) {
        let buf = ctx.global().buffers.get_mut(buf_id).unwrap();
        let amount_read = buf.read(data);
        if buf.is_empty() {
            ctx.lower_polled(read_polled);
        }
        ctx.raise_polled(write_polled);

        return amount_read as libc::ssize_t
    }

    if regular.peer.is_none() {
        return 0; // No more information to write to the connected socket
    };

    if is_nonblocking {
        unsafe { *libc::__errno_location() = libc::EAGAIN };
        return -1
    }

    // Our peer is still connected, and we're in blocking mode
    ctx.poll_until_ready(read_polled);

    let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
    let amount_read = recv_buf.read(data) as libc::ssize_t;

    if recv_buf.is_empty() {
        ctx.lower_polled(read_polled);
    }
    ctx.raise_polled(write_polled);

    amount_read
}

fn recv_connectionless_socket(
    ctx: &mut FizzleContext,
    data: &mut [u8],
    socket_id: SocketId,
    is_nonblocking: bool,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> libc::ssize_t {
    let SocketState::Connectionless(ConnectionlessSocket { backend: IoBackend::Regular(regular), .. }) = ctx.global().sockets.get(socket_id).unwrap()
    else {
        unreachable!()
    };

    let buf_id = regular.recv_buf;
    let write_polled = regular.write_polled;
    let read_polled = regular.read_polled;

    if !ctx.polled_is_ready(write_polled) {
        if is_nonblocking {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            return -1
        } else {
            ctx.poll_until_ready(write_polled);
        }
    }

    let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
    let read_len = read_datagram(recv_buf, data, addr, addrlen);

    if recv_buf.is_empty() {
        ctx.lower_polled(read_polled);
    }
    ctx.raise_polled(write_polled);

    read_len
}
