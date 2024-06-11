use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::{array, cmp, mem, ptr, slice};

use fizzle_common::io::TransportAddress;
use fizzle_common::storage::Buffer;

use crate::{hook_macros, state};
use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::state::backend::{ConnectedBackend, ConnectionlessBackend, FileBackend, IoBackend, RegularConnected, StdioBackend};
use crate::state::identifiers::SocketId;
use crate::state::{ConnectedSocket, ConnectionlessSocket, FuzzEndpointInfo, PipeMode, SocketLocationInfo, SocketState};
use crate::state::fd::FdResource;
use crate::state::identifiers::DescriptorId;

const PIPE_BUF: usize = 4096;
const IOV_MAX: usize = 16;

hook_macros::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_write(ctx) {

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
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
            FdResource::File(file_id) => match ctx.global.files.get(file_id).unwrap() {
                FileBackend::Passthrough => hook_macros::real!(write)(fd, buf, len),
                FileBackend::Peered(_) => unreachable!(),
                FileBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let write_polled = feedback.write_polled;
                    let read_polled = feedback.read_polled;

                    let event_raised = ctx.global.polled_events.get(write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(write_polled);
                    }
                    ctx.raise_polled(read_polled);

                    return written as isize
                }
                FileBackend::Plugin(plugin_id) => {
                    let plugin_id = *plugin_id;
                    let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
                    let buffer_id = plugin_info.write_buf;
                    let write_polled = plugin_info.write_polled;

                    let event_raised = ctx.global.polled_events.get(write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(write_polled);
                    }

                    return written as isize
                },
                FileBackend::Sink => len as libc::ssize_t,
                FileBackend::NullSink => len as libc::ssize_t,
                FileBackend::Fuzz => len as libc::ssize_t,
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => {
                let Some(peer_id) = ctx.global.pipes.get(pipe_id).unwrap().peer else {
                    *libc::__errno_location() = libc::EPIPE;
                    return -1
                };

                let peer_info = ctx.global.pipes.get(peer_id).unwrap();
                let buffer_id = peer_info.read_buf;
                let write_polled = peer_info.write_polled;
                let read_polled = peer_info.read_polled;

                let pipe_mode = peer_info.mode;

                let polled_is_ready = ctx.polled_is_ready(write_polled);
                drop(ctx);
                if !polled_is_ready {
                    if is_nonblocking {
                        unsafe { *libc::__errno_location() = libc::EAGAIN };
                        return -1
                    } else {
                        state::FIZZLE_STATE.poll_until_ready(write_polled);
                    }
                }

                let mut ctx = state::FIZZLE_STATE.acquire();

                // We need to verify that this connection has not shut down before writing to the same buffer_id
                if ctx.global.pipes.get(pipe_id).unwrap().peer.is_none() {
                    unsafe { *libc::__errno_location() = libc::EPIPE };
                    return -1
                };

                let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                let amount_written = match pipe_mode {
                    PipeMode::Direct => {
                        let packet_len = cmp::min(data.len(), 4096);
                        buf.write(&(packet_len as u16).to_be_bytes());
                        buf.write(&data[..packet_len])
                    },
                    PipeMode::Streamed => buf.write(data),
                };

                let buf_is_full = match pipe_mode {
                    PipeMode::Direct => FIZZLE_BUFFER_LENGTH - buf.len() < PIPE_BUF + 2,
                    PipeMode::Streamed => buf.is_full(),
                };

                if buf_is_full {
                    ctx.lower_polled(write_polled);
                }
                ctx.raise_polled(read_polled);

                amount_written as isize
            },
            FdResource::Stdin => 0,
            FdResource::Stdout => match ctx.global.stdio {
                StdioBackend::Passthrough => unreachable!(),
                StdioBackend::Peered(_) => unreachable!(),
                StdioBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let write_polled = feedback.write_polled;
                    let read_polled = feedback.read_polled;

                    let event_raised = ctx.global.polled_events.get(write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(write_polled);
                    }
                    ctx.raise_polled(read_polled);

                    written as isize
                },
                StdioBackend::Plugin(plugin_id) => {
                    let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
                    let buffer_id = plugin_info.write_buf;
                    let write_polled = plugin_info.write_polled;

                    let event_raised = ctx.global.polled_events.get(write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(write_polled);
                    }

                    written as isize
                },
                StdioBackend::Sink => len as libc::ssize_t,
                StdioBackend::NullSink => len as libc::ssize_t,
                StdioBackend::Fuzz => len as libc::ssize_t,
            },
            FdResource::Stderr => len as libc::ssize_t, // Transparently consume `stderr` output
            FdResource::Socket(socket_id) => match ctx.global.sockets.get(socket_id).unwrap() {
                SocketState::Connected(_) => {
                    drop(ctx);
                    send_connected_socket(&[data], socket_id, is_nonblocking)
                }
                SocketState::Connectionless(_) => {
                    drop(ctx);
                    send_connectionless_socket(data, socket_id, is_nonblocking, None)
                }
                _ => {
                    *libc::__errno_location() = libc::ENOTCONN;
                    -1
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

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);
        let data = slice::from_raw_parts(buf as *const u8, len);

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(socket_id).unwrap() {
            SocketState::Connected(_) => {
                drop(ctx);
                send_connected_socket(&[data], socket_id, is_nonblocking)
            }
            SocketState::Connectionless(_) => {
                drop(ctx);
                send_connectionless_socket(data, socket_id, is_nonblocking, None)
            }
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

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        let data = slice::from_raw_parts(buf as *const u8, len);

        match ctx.global.sockets.get(socket_id).unwrap() {
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

                drop(ctx);
                send_connectionless_socket(data, socket_id, is_nonblocking, addr)
            }
            SocketState::Connected(_) => {
                if addrlen != 0 || !dest_addr.is_null() {
                    *libc::__errno_location() = libc::EISCONN;
                    return -1
                }
                drop(ctx);
                send_connected_socket(&[data], socket_id, is_nonblocking)
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
    ) -> libc::ssize_t => fizzle_sendmsg(ctx) {

        if (flags & libc::MSG_FASTOPEN) != 0 {
            crate::report_strict_failure("fizzle does not currently implement TCP Fast Open")
        }

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);

        let slice_cnt = (*msg).msg_iovlen;
        let slices: [&[u8]; IOV_MAX] = array::from_fn(|i| {
            if i < slice_cnt {
                let iov = (*msg).msg_iov.add(i);
                slice::from_raw_parts((*iov).iov_base as *const u8, (*iov).iov_len)
            } else {
                &[]
            }
        });

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(socket_id).unwrap() {
            SocketState::Connected(_) => {
                drop(ctx);
                send_connected_socket(&slices[..slice_cnt], socket_id, is_nonblocking)
            }
            SocketState::Connectionless(_) => {
                drop(ctx);
                todo!()
                //send_connectionless_socket(data, socket_id, is_nonblocking, None)
            }
            _ => {
                *libc::__errno_location() = libc::ENOTCONN;
                return -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sendmmsg(
        fd: libc::c_int,
        msgvec: *mut libc::msghdr,
        vlen: libc::c_uint,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_sendmmsg(_ctx) {

        if (flags & libc::MSG_FASTOPEN) != 0 {
            crate::report_strict_failure("fizzle does not currently implement TCP Fast Open")
        }

        crate::report_strict_failure("`sendmsg` unimplemented");
        hook_macros::real!(sendmmsg)(fd, msgvec, vlen, flags)
    }
}

hook_macros::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_read(ctx) {

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
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
            FdResource::File(file_id) => match ctx.global.files.get(file_id).unwrap() {
                FileBackend::Passthrough => hook_macros::real!(read)(fd, buf, len),
                FileBackend::Peered(_) => unreachable!(),
                FileBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let read_polled = feedback.read_polled;
                    let write_polled = feedback.write_polled;

                    let event_raised = ctx.global.polled_events.get(read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(read_polled);
                    }
                    ctx.raise_polled(write_polled);

                    read_len as isize
                },
                FileBackend::Plugin(plugin_id) => {
                    let plugin_id = *plugin_id;
                    let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
                    let buffer_id = plugin_info.read_buf;
                    let read_polled = plugin_info.read_polled;

                    let event_raised = ctx.global.polled_events.get(read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(read_polled);
                    }

                    read_len as isize
                },
                FileBackend::Sink => 0 as libc::ssize_t,
                FileBackend::NullSink => {
                    for b in data.iter_mut() {
                        *b = 0;
                    }
                    data.len() as libc::ssize_t
                },
                FileBackend::Fuzz => {
                    let resource = fd_info.resource;
                    let &FuzzEndpointInfo { read_idx, read_polled } = ctx.global.fuzz_endpoints.get(&resource).unwrap();

                    let polled_is_ready = ctx.polled_is_ready(read_polled);
                    drop(ctx);

                    if !polled_is_ready {
                        if is_nonblocking {
                            *libc::__errno_location() = libc::EAGAIN;
                            return -1
                        } else {

                            state::FIZZLE_STATE.poll_until_ready(read_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();
                    
                    let total_fuzz_len = ctx.global.fuzz_input.len();
                    let read_len = cmp::min(total_fuzz_len - read_idx, len);
                    data[..read_len].copy_from_slice(&ctx.global.fuzz_input.data()[read_idx..read_idx + read_len]);
                    
                    let fuzz_endpoint = ctx.global.fuzz_endpoints.get_mut(&resource).unwrap();
                    let read_polled = fuzz_endpoint.read_polled;
                    fuzz_endpoint.read_idx += read_len;
                    if fuzz_endpoint.read_idx == total_fuzz_len {
                        ctx.lower_polled(read_polled);
                    }

                    read_len as libc::ssize_t
                },
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => {
                let pipe_info = ctx.global.pipes.get(pipe_id).unwrap();
                let peer_is_closed = pipe_info.peer.is_none();

                let buffer_id = pipe_info.read_buf;
                let write_polled = pipe_info.write_polled;
                let read_polled = pipe_info.read_polled;

                let pipe_mode = pipe_info.mode;
                let polled_is_ready = ctx.polled_is_ready(write_polled);
                drop(ctx);

                if !polled_is_ready {
                    if peer_is_closed {
                        return 0
                    } else if is_nonblocking {
                        unsafe { *libc::__errno_location() = libc::EAGAIN };
                        return -1
                    } else {
                        state::FIZZLE_STATE.poll_until_ready(write_polled);
                    }
                }

                let mut ctx = state::FIZZLE_STATE.acquire();

                if ctx.global.pipes.get(pipe_id).unwrap().peer.is_none() {
                    return 0
                }

                let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                let amount_read = match pipe_mode {
                    PipeMode::Direct => {
                        let mut packet_len_bytes = [0u8; 2];
                        assert!(buf.read(&mut packet_len_bytes) == 2);
                        buf.read(&mut data[..cmp::min(u16::from_be_bytes(packet_len_bytes) as usize, PIPE_BUF)])
                    },
                    PipeMode::Streamed => buf.read(data),
                };

                if buf.is_empty() {
                    ctx.lower_polled(read_polled);
                }
                ctx.raise_polled(write_polled);

                return amount_read as isize
            },
            FdResource::Stdin => match ctx.global.stdio {
                StdioBackend::Passthrough => unreachable!(),
                StdioBackend::Peered(_) => unreachable!(),
                StdioBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf;
                    let read_polled = feedback.read_polled;
                    let write_polled = feedback.write_polled;

                    let event_raised = ctx.global.polled_events.get(read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(read_polled);
                    }
                    ctx.raise_polled(write_polled);

                    read_len as isize
                },
                StdioBackend::Plugin(plugin_id) => {
                    let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
                    let buffer_id = plugin_info.write_buf;
                    let read_polled = plugin_info.read_polled;

                    let event_raised = ctx.global.polled_events.get(read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(read_polled);
                    }

                    read_len as isize
                },
                StdioBackend::Sink => 0,
                StdioBackend::NullSink => {
                    for b in data.iter_mut() {
                        *b = 0;
                    }
                    data.len() as libc::ssize_t
                },
                StdioBackend::Fuzz => {
                    let resource = fd_info.resource;
                    let &FuzzEndpointInfo { read_idx, read_polled } = ctx.global.fuzz_endpoints.get(&fd_info.resource).unwrap();

                    let polled_is_ready = ctx.polled_is_ready(read_polled);
                    drop(ctx);

                    if !polled_is_ready {
                        if is_nonblocking {
                            *libc::__errno_location() = libc::EAGAIN;
                            return -1
                        } else {

                            state::FIZZLE_STATE.poll_until_ready(read_polled);
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let total_fuzz_len = ctx.global.fuzz_input.len();
                    let read_len = cmp::min(ctx.global.fuzz_input.len() - read_idx, len);
                    data[..read_len].copy_from_slice(&ctx.global.fuzz_input.data()[read_idx..read_idx + read_len]);

                    let fuzz_endpoint = ctx.global.fuzz_endpoints.get_mut(&resource).unwrap();
                    let read_polled = fuzz_endpoint.read_polled;
                    fuzz_endpoint.read_idx += read_len;
                    if fuzz_endpoint.read_idx == total_fuzz_len {
                        ctx.lower_polled(read_polled);
                    }

                    read_len as libc::ssize_t
                },
            },
            FdResource::Stdout => 0,
            FdResource::Stderr => 0,
            FdResource::Socket(socket_id) => match ctx.global.sockets.get(socket_id).unwrap() {
                SocketState::Connected(_) => {
                    drop(ctx);
                    recv_connected_socket(&mut [data], socket_id, is_nonblocking)
                }
                SocketState::Connectionless(_) => {
                    drop(ctx);
                    recv_connectionless_socket(data, socket_id, is_nonblocking, ptr::null_mut(), ptr::null_mut())
                }
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

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);
        let data = slice::from_raw_parts_mut(buf as *mut u8, len);

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(socket_id).unwrap() {
            SocketState::Connected(conn_info) => {
                crate::encode_inet_address(src_addr, conn_info.rem_addr.address()); // TODO: buffer overflow if addrlen is too short...
                drop(ctx);
                recv_connected_socket(&mut [data], socket_id, is_nonblocking)
            },
            SocketState::Connectionless(_) => {
                drop(ctx);
                recv_connectionless_socket(data, socket_id, is_nonblocking, src_addr, addrlen)
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
    ) -> libc::ssize_t => fizzle_recvmsg(ctx) {

        // TODO: have to fill out all the other fields in `msg`
        // For now we do this:
        (*msg).msg_controllen = 0;
        (*msg).msg_flags = 0;

        if !(*msg).msg_name.is_null() {
            panic!("recvmsg unimplemented for receiving message name");
        }

        let Some(fd_info) = ctx.local.fds.get(DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);

        let slice_cnt = (*msg).msg_iovlen;
        let mut slices: [&mut [u8]; IOV_MAX] = array::from_fn(|i| {
            if i < slice_cnt {
                let iov = (*msg).msg_iov.add(i);
                slice::from_raw_parts_mut((*iov).iov_base as *mut u8, (*iov).iov_len)
            } else {
                &mut []
            }
        });

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(socket_id).unwrap() {
            SocketState::Connected(_conn_info) => {
                drop(ctx);
                recv_connected_socket(&mut slices[..slice_cnt], socket_id, is_nonblocking)
            },
            SocketState::Connectionless(_) => {
                drop(ctx);
                todo!()
                //recv_connectionless_socket(data, socket_id, is_nonblocking, src_addr, addrlen)
            },
            _ => {
                *libc::__errno_location() = libc::ENOTCONN;
                return -1
            }
        }
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

hook_macros::hook! {
    unsafe fn pread(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pread(_ctx) {

        crate::report_strict_failure("`pread` unimplemented");
        hook_macros::real!(pread)(fd, buf, count, offset)
    }
}

hook_macros::hook! {
    unsafe fn pwrite(
        fd: libc::c_int,
        buf: *const libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pwrite(_ctx) {

        crate::report_strict_failure("`pwrite` unimplemented");
        hook_macros::real!(pwrite)(fd, buf, count, offset)
    }
}

hook_macros::hook! {
    unsafe fn readv(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int
    ) -> libc::ssize_t => fizzle_readv(_ctx) {

        crate::report_strict_failure("`readv` unimplemented");
        hook_macros::real!(readv)(fd, iov, iovcnt)
    }
}

hook_macros::hook! {
    unsafe fn writev(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int
    ) -> libc::ssize_t => fizzle_writev(_ctx) {

        crate::report_strict_failure("`writev` unimplemented");
        hook_macros::real!(writev)(fd, iov, iovcnt)
    }
}

hook_macros::hook! {
    unsafe fn preadv(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_preadv(_ctx) {

        crate::report_strict_failure("`preadv` unimplemented");
        hook_macros::real!(preadv)(fd, iov, iovcnt, offset)
    }
}

hook_macros::hook! {
    unsafe fn pwritev(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pwritev(_ctx) {

        crate::report_strict_failure("`pwritev` unimplemented");
        hook_macros::real!(pwritev)(fd, iov, iovcnt, offset)
    }
}

hook_macros::hook! {
    unsafe fn preadv2(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_preadv2(_ctx) {

        crate::report_strict_failure("`preadv2` unimplemented");
        hook_macros::real!(preadv2)(fd, iov, iovcnt, offset, flags)
    }
}

hook_macros::hook! {
    unsafe fn pwritev2(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_pwritev2(_ctx) {

        crate::report_strict_failure("`pwritev2` unimplemented");
        hook_macros::real!(pwritev2)(fd, iov, iovcnt, offset, flags)
    }
}

fn write_datagram<const N: usize>(
    send_buf: &mut Buffer<N>,
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
    data: &[&[u8]],
    socket_id: SocketId,
    is_nonblocking: bool,
) -> libc::ssize_t {
    let mut ctx = state::FIZZLE_STATE.acquire();

    let SocketState::Connected(ConnectedSocket { backend: ConnectedBackend::Peered(regular), .. }) = ctx.global.sockets.get(socket_id).unwrap() else {
        unreachable!()
    };

    let Some(peer) = regular.peer else {
        return 0; // No more information to write to the connected socket
    };

    let Some(SocketState::Connected(peer_info)) = ctx.global.sockets.get(peer) else {
        unreachable!()
    };

    // TODO: potentially make this more DRY
    match peer_info.backend {
        ConnectedBackend::Passthrough => unimplemented!(),
        ConnectedBackend::Peered(regular_peer) => {
            let buffer_id = regular_peer.recv_buf;
            let write_polled = regular_peer.write_polled;
            let read_polled = regular_peer.read_polled;

            let polled_is_ready = ctx.polled_is_ready(write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket { backend: IoBackend::Peered(RegularConnected { peer: Some(_), .. }), .. })) = ctx.global.sockets.get(socket_id) else {
                return 0
            };

            let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
            let mut total_written = 0;
            for slice in data.iter() {
                let written = buf.write(slice);
                if written > 0 {
                    total_written += written;
                }

                if written != slice.len() {
                    break
                }
            }

            if buf.is_full() {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return total_written as isize
        },
        ConnectedBackend::Feedback(feedback) => {
            let buffer_id = feedback.buf;
            let write_polled = feedback.write_polled;
            let read_polled = feedback.read_polled;

            let polled_is_ready = ctx.polled_is_ready(write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket { backend: IoBackend::Peered(RegularConnected { peer: Some(_), .. }), .. })) = ctx.global.sockets.get(socket_id) else {
                return 0
            };

            let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
            let mut total_written = 0;
            for slice in data.iter() {
                let written = buf.write(slice);
                if written > 0 {
                    total_written += written;
                }

                if written != slice.len() {
                    break
                }
            }

            if buf.is_full() {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return total_written as isize
        },
        ConnectedBackend::Plugin(plugin_id) => {
            let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
            let buffer_id = plugin_info.write_buf;
            let write_polled = plugin_info.write_polled;
            let read_polled = plugin_info.read_polled;

            let polled_is_ready = ctx.polled_is_ready(write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket { backend: IoBackend::Peered(RegularConnected { peer: Some(_), .. }), .. })) = ctx.global.sockets.get(socket_id) else {
                return 0
            };

            let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
            let mut total_written = 0;
            for slice in data.iter() {
                let written = buf.write(slice);
                if written > 0 {
                    total_written += written;
                }

                if written != slice.len() {
                    break
                }
            }

            if buf.is_full() {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return total_written as isize
        },
        ConnectedBackend::Sink => data.len() as libc::ssize_t,
        ConnectedBackend::NullSink => data.len() as libc::ssize_t,
        ConnectedBackend::Fuzz => data.len() as libc::ssize_t,
    }
}

const MAX_DATAGRAM: usize = 65507;
const MAX_INTERNAL_DATAGRAM: usize = MAX_DATAGRAM + mem::size_of::<libc::sockaddr_storage>() + 3;

fn send_connectionless_socket(
    data: &[u8],
    socket_id: SocketId,
    is_nonblocking: bool,
    addr: Option<SocketAddr>,
) -> libc::ssize_t {

    let mut ctx = state::FIZZLE_STATE.acquire();

    let SocketState::Connectionless(sock_info) = ctx.global.sockets.get(socket_id).unwrap()
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

    let local_addr = sock_info.local_addr;

    let Some(SocketLocationInfo {
        bound_socket: Some(peer_sock_id),
        ..
    }) = ctx
        .global
        .socket_locations
        .get(&TransportAddress::Udp(rem_addr))
    else {
        unsafe { *libc::__errno_location() = libc::ECONNRESET }; // No socket was listening at the endpoint
        return -1; // TODO: should we just return data.len() here instead?
    };
    let peer_sock_id = *peer_sock_id;

    let SocketState::Connectionless(peer_info) = ctx.global.sockets.get(peer_sock_id).unwrap()
    else {
        unreachable!()
    };

    match peer_info.backend {
        ConnectionlessBackend::Passthrough => unimplemented!(),
        ConnectionlessBackend::Peered(regular_peer) => {
            let write_polled = regular_peer.write_polled;

            let polled_is_ready = ctx.polled_is_ready(write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketLocationInfo {
                bound_socket: Some(peer_sock_id),
                ..
            }) = ctx
                .global
                .socket_locations
                .get(&TransportAddress::Udp(rem_addr))
            else {
                return data.len() as libc::ssize_t // Drop packet
            };

            let peer_sock_id = *peer_sock_id;


            let SocketState::Connectionless(ConnectionlessSocket { backend: IoBackend::Peered(regular_peer), .. }) = ctx.global.sockets.get(peer_sock_id).unwrap() else {
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

            let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
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

            let polled_is_ready = ctx.polled_is_ready(write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We don't need to verify that this connection has not shut down, as it's a Feedback endpoint

            let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &local_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(write_polled);
            }
            ctx.raise_polled(read_polled);

            return amount_written as isize
        },
        ConnectionlessBackend::Plugin(plugin_id) => {
            let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
            let buffer_id = plugin_info.write_buf;
            let write_polled = plugin_info.write_polled;

            let polled_is_ready = ctx.polled_is_ready(write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We don't need to verify that this connection has not shut down, as it's a Plugin endpoint

            let buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &rem_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(write_polled);
            }

            return amount_written as isize
        },
        ConnectionlessBackend::Sink => data.len() as libc::ssize_t,
        ConnectionlessBackend::NullSink => data.len() as libc::ssize_t,
        ConnectionlessBackend::Fuzz => data.len() as libc::ssize_t,
    }
}

fn read_datagram<const N: usize>(
    recv_buf: &mut Buffer<N>,
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
    data: &mut [&mut [u8]],
    socket_id: SocketId,
    is_nonblocking: bool,
) -> libc::ssize_t {

    let mut ctx = state::FIZZLE_STATE.acquire();

    let SocketState::Connected(sock_info) = ctx.global.sockets.get(socket_id).unwrap() else {
        panic!("internal error")
    };

    match sock_info.backend {
        IoBackend::Passthrough => unimplemented!(),
        IoBackend::Peered(regular) => {
            let buf_id = regular.recv_buf;
            let write_polled = regular.write_polled;
            let read_polled = regular.read_polled;

            // First, check to see if we can just immediately read despite the peer being closed
            if ctx.polled_is_ready(read_polled) {
                let recv_buf = ctx.global.buffers.get_mut(buf_id).unwrap();
                let mut total_read = 0;
                for slice in data.iter_mut() {
                    let read_amount = recv_buf.read(slice);
                    if read_amount > 0 {
                        total_read += read_amount;
                    }

                    if read_amount != slice.len() {
                        break
                    }
                }

                if recv_buf.is_empty() {
                    ctx.lower_polled(read_polled);
                }
                ctx.raise_polled(write_polled);

                return total_read as libc::ssize_t
            }

            if regular.peer.is_none() {
                return 0; // No more information to write to the connected socket
            };

            if is_nonblocking {
                unsafe { *libc::__errno_location() = libc::EAGAIN };
                return -1
            }

            // Our peer is still connected, and we're in blocking mode
            drop(ctx);
            state::FIZZLE_STATE.poll_until_ready(read_polled);
            let mut ctx = state::FIZZLE_STATE.acquire();

            let recv_buf = ctx.global.buffers.get_mut(buf_id).unwrap();
            let mut total_read = 0;
            for slice in data.iter_mut() {
                let read_amount = recv_buf.read(slice);
                if read_amount > 0 {
                    total_read += read_amount;
                }

                if read_amount != slice.len() {
                    break
                }
            }

            if recv_buf.is_empty() {
                ctx.lower_polled(read_polled);
            }
            ctx.raise_polled(write_polled);

            total_read as libc::ssize_t
        },
        IoBackend::Feedback(feedback_info) => {
            let buf_id = feedback_info.buf;
            let write_polled = feedback_info.write_polled;
            let read_polled = feedback_info.read_polled;

            // First, check to see if we can just immediately read despite the peer being closed
            if ctx.polled_is_ready(read_polled) {
                let recv_buf = ctx.global.buffers.get_mut(buf_id).unwrap();
                let mut total_read = 0;
                for slice in data.iter_mut() {
                    let read_amount = recv_buf.read(slice);
                    if read_amount > 0 {
                        total_read += read_amount;
                    }

                    if read_amount != slice.len() {
                        break
                    }
                }

                if recv_buf.is_empty() {
                    ctx.lower_polled(read_polled);
                }
                ctx.raise_polled(write_polled);

                return total_read as libc::ssize_t
            }

            if is_nonblocking {
                unsafe { *libc::__errno_location() = libc::EAGAIN };
                return -1
            }

            // Our peer is still connected, and we're in blocking mode
            drop(ctx);
            state::FIZZLE_STATE.poll_until_ready(read_polled);
            let mut ctx = state::FIZZLE_STATE.acquire();

            let recv_buf = ctx.global.buffers.get_mut(buf_id).unwrap();
            let mut total_read = 0;
            for slice in data.iter_mut() {
                let read_amount = recv_buf.read(slice);
                if read_amount > 0 {
                    total_read += read_amount;
                }

                if read_amount != slice.len() {
                    break
                }
            }

            if recv_buf.is_empty() {
                ctx.lower_polled(read_polled);
            }
            ctx.raise_polled(write_polled);

            total_read as libc::ssize_t
        },
        IoBackend::Plugin(plugin_id) => {
            let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
            let buffer_id = plugin_info.read_buf;
            let read_polled = plugin_info.read_polled;

            let event_raised = ctx.global.polled_events.get(read_polled).unwrap().event_raised;
            drop(ctx);

            if !event_raised {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(read_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            let recv_buf = ctx.global.buffers.get_mut(buffer_id).unwrap();
            let mut total_read = 0;
            for slice in data.iter_mut() {
                let read_amount = recv_buf.read(slice);
                if read_amount > 0 {
                    total_read += read_amount;
                }

                if read_amount != slice.len() {
                    break
                }
            }

            if recv_buf.is_empty() {
                ctx.lower_polled(read_polled);
            }

            return total_read as libc::ssize_t
        },
        IoBackend::Sink => return 0 as libc::ssize_t,
        IoBackend::NullSink => {
            let mut total_len = 0;
            for slice in data.iter_mut() {
                for b in slice.iter_mut() {
                    *b = 0;
                }
                total_len += slice.len();
            }
            
            total_len as libc::ssize_t
        },
        IoBackend::Fuzz => {
            let resource = FdResource::Socket(socket_id);
            let &FuzzEndpointInfo { mut read_idx, read_polled } = ctx.global.fuzz_endpoints.get(&resource).unwrap();

            let polled_is_ready = ctx.polled_is_ready(read_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1
                } else {
                    state::FIZZLE_STATE.poll_until_ready(read_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();
            
            let total_fuzz_len = ctx.global.fuzz_input.len();
            let start_read_idx = read_idx;

            for slice in data.iter_mut() {
                let read_len = cmp::min(total_fuzz_len - read_idx, slice.len());
                slice[..read_len].copy_from_slice(&ctx.global.fuzz_input.data()[read_idx..read_idx + read_len]);
                read_idx += read_len;
                if read_idx == total_fuzz_len {
                    break
                }
            }

            let fuzz_endpoint = ctx.global.fuzz_endpoints.get_mut(&resource).unwrap();
            let read_polled = fuzz_endpoint.read_polled;
            fuzz_endpoint.read_idx = read_idx;
            if fuzz_endpoint.read_idx == total_fuzz_len {
                ctx.lower_polled(read_polled);
            }

            (read_idx - start_read_idx) as libc::ssize_t
        },
    }
}

fn recv_connectionless_socket(
    data: &mut [u8],
    socket_id: SocketId,
    is_nonblocking: bool,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> libc::ssize_t {

    let mut ctx = state::FIZZLE_STATE.acquire();

    // TODO: this isn't finished...
    let SocketState::Connectionless(ConnectionlessSocket { backend: IoBackend::Peered(regular), .. }) = ctx.global.sockets.get(socket_id).unwrap()
    else {
        unreachable!()
    };

    let buf_id = regular.recv_buf;
    let write_polled = regular.write_polled;
    let read_polled = regular.read_polled;

    let polled_is_ready = ctx.polled_is_ready(write_polled);
    drop(ctx);

    if !polled_is_ready {
        if is_nonblocking {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            return -1
        } else {
            state::FIZZLE_STATE.poll_until_ready(write_polled);
        }
    }

    let mut ctx = state::FIZZLE_STATE.acquire();

    let recv_buf = ctx.global.buffers.get_mut(buf_id).unwrap();
    let read_len = read_datagram(recv_buf, data, addr, addrlen);

    if recv_buf.is_empty() {
        ctx.lower_polled(read_polled);
    }
    ctx.raise_polled(write_polled);

    read_len
}


