use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::{cmp, mem, ptr, slice};

use fizzle_common::io::TransportAddress;
use fizzle_common::storage::RingBuffer;

use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::state::identifiers::SocketId;
use crate::state::{ConnectedPeer, ConnectedSocket, FizzleContext, IoBackend, SocketLocationInfo};
use crate::{
    hook_macros,
    state::{fd::FdResource, identifiers::DescriptorId, SocketState},
};

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
            FdResource::Directory(_) => {
                *libc::__errno_location() = libc::EBADF;
                return -1
            },
            FdResource::File(file_id) => match ctx.global().files.get(file_id).unwrap().backend {
                IoBackend::Feedback(_) => todo!(),
                IoBackend::Plugin(_) => todo!(),
                IoBackend::Sink => len as libc::ssize_t,
                IoBackend::NullSink => len as libc::ssize_t,
                IoBackend::Fuzz => len as libc::ssize_t,
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::PassthroughFile => hook_macros::real!(write)(fd, buf, len),
            FdResource::Pipe(_) => todo!(),
            FdResource::Stdin => todo!(),
            FdResource::Stdout => todo!(),
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
        let data = slice::from_raw_parts(buf as *const u8, len);

        match fd_info.resource {
            FdResource::Directory(_) => {
                *libc::__errno_location() = libc::EBADF;
                return -1
            },
            FdResource::File(file_id) => match ctx.global().files.get(file_id).unwrap().backend {
                IoBackend::Feedback(_) => todo!(),
                IoBackend::Plugin(_) => todo!(),
                IoBackend::Sink => 0,
                IoBackend::NullSink => {
                    libc::memset(buf, 0, len);
                    len as libc::ssize_t
                },
                IoBackend::Fuzz => todo!(),
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::PassthroughFile => hook_macros::real!(write)(fd, buf, len),
            FdResource::Pipe(_) => todo!(),
            FdResource::Stdin => todo!(),
            FdResource::Stdout => todo!(),
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
    let Ok(1) = send_buf.write(slice::from_ref(&sockaddr_len)) else {
        panic!()
    };

    match send_buf.write(sockaddr_bytes) {
        Ok(len) if len == sockaddr_len as usize => (),
        _ => panic!(),
    }

    let data_len = data.len() as u16;
    let Ok(2) = send_buf.write(&data_len.to_be_bytes()) else {
        panic!()
    };

    match send_buf.write(data) {
        Ok(len) if len == data.len() => (),
        _ => panic!(),
    }

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

    match sock_info.peer {
        Some(ConnectedPeer::Socket(peer_id)) => {
            let Some(SocketState::Connected(peer_info)) = ctx.global().sockets.get(peer_id) else {
                return 0; // No more information to write to the connected socket
            };

            let buf_id = peer_info.recv_buf;
            let write_polled_id = peer_info.write_polled;
            let read_polled_id = peer_info.read_polled;

            if is_nonblocking {
                if ctx.polled_is_ready(write_polled_id) {
                    ctx.raise_polled(read_polled_id);
                    let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                    let written = recv_buf.write(data).unwrap() as libc::ssize_t;
                    if recv_buf.is_full() {
                        ctx.lower_polled(write_polled_id);
                    }
                    ctx.raise_polled(read_polled_id);
                    written
                } else {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    -1
                }
            } else {
                ctx.poll_until_ready(write_polled_id);

                // We need to check here to see if the peer has shutdown or closed
                let SocketState::Connected(ConnectedSocket { peer: Some(_), .. }) =
                    ctx.global().sockets.get(socket_id).unwrap()
                else {
                    return 0; // The peer has shutdown
                };

                let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                let written = recv_buf.write(data).unwrap() as libc::ssize_t;

                let recv_buf_full = recv_buf.is_full();
                if recv_buf_full {
                    ctx.lower_polled(write_polled_id);
                } else {
                    ctx.enqueue_next_polled(write_polled_id);
                }

                ctx.raise_polled(read_polled_id);

                written
            }
        }
        Some(ConnectedPeer::Emulated(IoBackend::Sink)) => data.len() as libc::ssize_t,
        Some(ConnectedPeer::Emulated(IoBackend::NullSink)) => data.len() as libc::ssize_t,
        Some(ConnectedPeer::Emulated(IoBackend::Fuzz)) => data.len() as libc::ssize_t,
        Some(ConnectedPeer::Emulated(IoBackend::Feedback(_))) => todo!(),
        Some(ConnectedPeer::Emulated(IoBackend::Plugin(plugin_id))) => {
            let plugin_info = ctx.global().plugins.get(plugin_id).unwrap();

            let buf_id = plugin_info.input;
            let write_polled_id = plugin_info.in_polled;

            if is_nonblocking {
                if ctx.polled_is_ready(write_polled_id) {
                    let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                    let written = recv_buf.write(data).unwrap() as libc::ssize_t;
                    if recv_buf.is_full() {
                        ctx.lower_polled(write_polled_id);
                    }
                    written
                } else {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    -1
                }
            } else {
                ctx.poll_until_ready(write_polled_id);
                // Emulated I/O backends don't shutdown so we don't need to check here
                let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                let written = recv_buf.write(data).unwrap() as libc::ssize_t;

                if recv_buf.is_full() {
                    ctx.lower_polled(write_polled_id);
                } else {
                    ctx.enqueue_next_polled(write_polled_id);
                }

                written
            }
        }
        None => 0,
    }
}

const MAX_INTERNAL_DATAGRAM: usize = 65507 + mem::size_of::<libc::sockaddr_storage>() + 3;

fn send_connectionless_socket(
    ctx: &mut FizzleContext,
    data: &[u8],
    socket_id: SocketId,
    is_nonblocking: bool,
    addr: Option<SocketAddr>,
) -> libc::ssize_t {
    let SocketState::Connectionless(sock_info) = ctx.global().sockets.get(socket_id).unwrap()
    else {
        panic!("internal error")
    };

    if data.len() > 65507 {
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
        unsafe { *libc::__errno_location() = libc::ECONNRESET }; // TODO: what happens when you send a packet to a non-listening UDP socket??
        return -1;
    };
    let peer_sock_id = *peer_sock_id;

    let SocketState::Connectionless(peer_info) = ctx.global().sockets.get(peer_sock_id).unwrap()
    else {
        panic!("internal fizzle error--UDP listening socket not in `Connectionless` state")
    };

    match peer_info.backend {
        None => {
            let buf_id = peer_info.recv_buf;
            let write_polled_id = peer_info.write_polled;
            let read_polled_id = peer_info.read_polled;

            if is_nonblocking {
                if ctx.polled_is_ready(write_polled_id) {
                    let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                    let written = write_datagram(recv_buf, data, &rem_addr);

                    let recv_buf_full =
                        (FIZZLE_BUFFER_LENGTH - recv_buf.len()) < MAX_INTERNAL_DATAGRAM;
                    if recv_buf_full {
                        ctx.lower_polled(write_polled_id);
                    }

                    ctx.raise_polled(read_polled_id);

                    written
                } else {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    -1
                }
            } else {
                // Normally a connectionless socket would drop select packets, but loopback UDP
                ctx.poll_until_ready(write_polled_id);

                let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                let written = write_datagram(recv_buf, data, &rem_addr);

                let recv_buf_full = (FIZZLE_BUFFER_LENGTH - recv_buf.len()) < MAX_INTERNAL_DATAGRAM;
                if recv_buf_full {
                    ctx.lower_polled(write_polled_id);
                } else {
                    ctx.enqueue_next_polled(write_polled_id);
                }

                ctx.raise_polled(read_polled_id);

                written
            }
        }
        Some(IoBackend::NullSink) => data.len() as libc::ssize_t,
        Some(IoBackend::Sink) => data.len() as libc::ssize_t,
        Some(IoBackend::Fuzz) => data.len() as libc::ssize_t,
        Some(IoBackend::Feedback(_)) => todo!(),
        Some(IoBackend::Plugin(plugin_id)) => {
            let plugin_info = ctx.global().plugins.get(plugin_id).unwrap();

            let buf_id = plugin_info.input;
            let write_polled_id = plugin_info.in_polled;

            if is_nonblocking {
                if ctx.polled_is_ready(write_polled_id) {
                    let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                    let written = recv_buf.write(data).unwrap() as libc::ssize_t;
                    let recv_buf_full =
                        (FIZZLE_BUFFER_LENGTH - recv_buf.len()) < MAX_INTERNAL_DATAGRAM;
                    if recv_buf_full {
                        ctx.lower_polled(write_polled_id);
                    }
                    written
                } else {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    -1
                }
            } else {
                ctx.poll_until_ready(write_polled_id);
                // Emulated I/O backends don't shutdown so we don't need to check here
                let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
                let written = recv_buf.write(data).unwrap() as libc::ssize_t;

                if recv_buf.is_full() {
                    ctx.lower_polled(write_polled_id);
                } else {
                    ctx.enqueue_next_polled(write_polled_id);
                }

                written
            }
        }
    }
}

fn read_datagram<const N: usize>(
    recv_buf: &mut RingBuffer<N>,
    data: &mut [u8],
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> libc::ssize_t {
    let mut stored_addrlen = 0u8;
    let Ok(1) = recv_buf.read(slice::from_mut(&mut stored_addrlen)) else {
        panic!()
    };

    let mut addr_buf = [0u8; 128];
    match recv_buf.read(&mut addr_buf[..stored_addrlen as usize]) {
        Ok(len) if len == stored_addrlen as usize => (),
        _ => panic!("fizzle datagram internal address stored incorrectly"),
    }

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
    let Ok(2) = recv_buf.read(&mut pktlen_buf) else {
        panic!("fizzle datagram internal address stored incorrectly")
    };

    let total_len = u16::from_be_bytes(pktlen_buf) as usize;
    let read_len = cmp::min(total_len, data.len());

    match recv_buf.read(&mut data[..read_len]) {
        Ok(len) if len == read_len => total_len as libc::ssize_t,
        _ => panic!("fizzle datagram internal bytes stored incorrectly"),
    }
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

    let buf_id = sock_info.recv_buf;
    let write_polled_id = sock_info.write_polled;
    let read_polled_id = sock_info.read_polled;

    let has_shutdown_peer = sock_info.peer.is_none();
    let buf_empty = ctx.global().buffers.get(buf_id).unwrap().is_empty();

    if has_shutdown_peer && buf_empty {
        return 0;
    }

    if is_nonblocking {
        if ctx.polled_is_ready(read_polled_id) {
            let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
            let read_len = recv_buf.read(data).unwrap() as libc::ssize_t;
            if recv_buf.is_empty() {
                ctx.lower_polled(read_polled_id);
            }
            read_len
        } else {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            -1
        }
    } else {
        ctx.poll_until_ready(read_polled_id);

        let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
        let read_len = recv_buf.read(data).unwrap() as libc::ssize_t;

        let recv_buf_empty = recv_buf.is_empty();
        let recv_buf_full = recv_buf.is_full();

        if !recv_buf_full {
            ctx.raise_polled(write_polled_id);
        }

        if recv_buf_empty {
            ctx.lower_polled(read_polled_id);
        } else {
            ctx.enqueue_next_polled(read_polled_id);
        }

        read_len
    }
}

fn recv_connectionless_socket(
    ctx: &mut FizzleContext,
    data: &mut [u8],
    socket_id: SocketId,
    is_nonblocking: bool,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> libc::ssize_t {
    let SocketState::Connectionless(sock_info) = ctx.global().sockets.get(socket_id).unwrap()
    else {
        panic!("internal error")
    };

    let buf_id = sock_info.recv_buf;
    let write_polled_id = sock_info.write_polled;
    let read_polled_id = sock_info.read_polled;

    if is_nonblocking {
        if ctx.polled_is_ready(read_polled_id) {
            let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
            let read_len = read_datagram(recv_buf, data, addr, addrlen);

            if recv_buf.is_empty() {
                ctx.lower_polled(read_polled_id);
            }
            ctx.raise_polled(write_polled_id);

            read_len
        } else {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            -1
        }
    } else {
        // Normally a connectionless socket would drop select packets, but loopback UDP
        ctx.poll_until_ready(read_polled_id);

        let recv_buf = ctx.global().buffers.get_mut(buf_id).unwrap();
        let read_len = read_datagram(recv_buf, data, addr, addrlen);

        if recv_buf.is_empty() {
            ctx.lower_polled(read_polled_id);
        } else {
            ctx.enqueue_next_polled(read_polled_id);
        }

        ctx.raise_polled(write_polled_id);

        read_len
    }
}
