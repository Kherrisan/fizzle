use std::mem::MaybeUninit;
use std::{array, cmp, mem, ptr, slice};

use fizzle_common::io::{AddressFamily, TransportAddress, TransportProtocol};
use fizzle_common::storage::{Buffer, Rc};

use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::state::backend::{
    ConnectedBackend, ConnectionlessBackend, FileBackend, IoBackend, RegularConnected, StdioBackend,
};
use crate::state::fd::FdResource;
use crate::state::identifiers::DescriptorId;
use crate::state::identifiers::SocketId;
use crate::state::{
    ConnectedSocket, ConnectionlessSocket, FuzzEndpointInfo, PipeMode, SocketLocationInfo,
    SocketState,
};
use crate::{hook_macros, state};

const PIPE_BUF: usize = 4096;
const IOV_MAX: usize = 16;

hook_macros::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_write(ctx) {

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
            log::warn!("write() called with unknown file descriptor");
            return hook_macros::real!(write)(fd, buf, len)
            /*
            *libc::__errno_location() = libc::EBADF;
            return -1
            */
        };

        let is_nonblocking = fd_info.nonblocking;
        let data = slice::from_raw_parts(buf as *const u8, len);

        match &fd_info.resource {
            FdResource::Epoll(_) | FdResource::Directory(_) => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            },
            FdResource::EventFd(eventfd_id) => {
                let eventfd_id = eventfd_id.clone();
                let Ok(increment) = data.try_into().map(u64::from_ne_bytes) else {
                    log::warn!("eventfd received `write` with invalid length != 8");
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                };

                if increment == u64::MAX {
                    log::warn!("eventfd received `write` with invalid value 0xffffffff");
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                let eventfd = ctx.global.event_fds.get(&eventfd_id).unwrap();
                let mut current_counter = eventfd.counter;
                let read_polled = eventfd.read_polled.clone();
                let write_polled = eventfd.write_polled.clone();

                drop(ctx);

                if is_nonblocking && current_counter.checked_add(increment + 1).is_none() {
                    *libc::__errno_location() = libc::EAGAIN;
                    return -1
                }

                // The following code is designed very specifically to handle polling for arbitrary
                // `increment` values. Specifically, an application may choose to increment an
                // eventfd by up to `u64::MAX - 1`; however, if such an increment would cause the
                // eventfd to exceed its maximum permittable value (which is also `u64::MAX - 1`),
                // then the write operation for that increment should block until it can succeed.
                // This is the challenge: how do we know when to raise/lower a poll for a variably
                // chosen increment value so that writes preceding or following a blocked write will
                // still succeed if they are of a sufficiently small increment value?a
                //
                // The solution is as follows: check initially to see if the write will succeed.
                // Note that this DOES NOT use `polled_is_ready()`, but rather directly checks the
                // counter value added with the increment. If this would overflow the maximum value,
                // lower `write_polled` and poll until it has been raised again. Then check again; 
                // continue this loop until succeeded.
                //
                // In the event that a large write blocks, smaller writes that would not overflow
                // the eventfd will still succeed, as this directly checks the addition value rather
                // than `polled_is_ready()` (`write_polled` will be lowered while a large write is
                // blocked). Whenever a read is performed, `write_polled` will be raised, triggering
                // an event for every poller waiting to write to the eventfd. This ensures that a
                // blocked writer will not remain blocked if the eventfd value drops low enough for 
                // the read to succeed. If the performed read does not drop the value low enough, or
                // if another blocked write is carried out in between the read and the blocked write
                // check, the writer will simply loop again and lower/re-poll `write_polled` so that
                // the next subsequent read will trigger a notification. This solution is a bit
                // "noisy", in that every read awakes all blocked writers to check for readiness
                // instead of one, but it's what I could come up with within the constraints of
                // Fizzle's current polling infrastructure.
                //
                // As a pleasent side note, the combination of this algorithm with the Vec data
                // structures we use for holding pollers within `polled` instances means that a
                // blocked write is guaranteed to always eventually be at the top of the queue
                // following each read, thereby ensuring no blocked write is starved.
                let new_counter = loop {
                    match current_counter.checked_add(increment) {
                        Some(c) if c != u64::MAX => break c,
                        _ => {
                            let mut ctx = state::FIZZLE_STATE.acquire();
                            ctx.lower_polled(&write_polled);
                            drop(ctx);

                            state::FIZZLE_STATE.poll_until_ready(write_polled.clone());

                            let ctx = state::FIZZLE_STATE.acquire();
                            current_counter = ctx.global.event_fds.get(&eventfd_id).unwrap().counter;
                            drop(ctx);
                        },
                    }
                };

                let mut ctx = state::FIZZLE_STATE.acquire();
                ctx.global.event_fds.get_mut(&eventfd_id).unwrap().counter = new_counter;
                ctx.raise_polled(&read_polled);
                if new_counter == u64::MAX - 1 {
                    ctx.lower_polled(&write_polled);
                }

                8
            }
            FdResource::File(file_id) => match ctx.global.files.get(&file_id).unwrap() {
                FileBackend::Passthrough => hook_macros::real!(write)(fd, buf, len),
                FileBackend::Peered(_) => unreachable!(),
                FileBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf.clone();
                    let write_polled = feedback.write_polled.clone();
                    let read_polled = feedback.read_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(&write_polled);
                    }
                    ctx.raise_polled(&read_polled);

                    return written as isize
                }
                FileBackend::Plugin(plugin_id) => {
                    let plugin_id = plugin_id.clone();
                    let plugin_info = ctx.global.plugins.get(&plugin_id).unwrap();
                    let buffer_id = plugin_info.write_buf.clone();
                    let write_polled = plugin_info.write_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(&write_polled);
                    }

                    return written as isize
                },
                FileBackend::Sink => len as libc::ssize_t,
                FileBackend::NullSink => len as libc::ssize_t,
                FileBackend::Fuzz(_) => len as libc::ssize_t,
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => {
                let pipe_id = pipe_id.clone();
                let Some(peer_id) = ctx.global.pipes.get(&pipe_id).unwrap().peer.clone() else {
                    *libc::__errno_location() = libc::EPIPE;
                    return -1
                };

                let peer_info = ctx.global.pipes.get(&peer_id).unwrap();
                let buffer_id = peer_info.read_buf.clone();
                let write_polled = peer_info.write_polled.clone();
                let read_polled = peer_info.read_polled.clone();

                let pipe_mode = peer_info.mode;

                let polled_is_ready = ctx.polled_is_ready(&write_polled);
                drop(ctx);
                if !polled_is_ready {
                    if is_nonblocking {
                        unsafe { *libc::__errno_location() = libc::EAGAIN };
                        return -1
                    } else {
                        state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                    }
                }

                let mut ctx = state::FIZZLE_STATE.acquire();

                // We need to verify that this connection has not shut down before writing to the same buffer_id
                if ctx.global.pipes.get(&pipe_id).unwrap().peer.is_none() {
                    unsafe { *libc::__errno_location() = libc::EPIPE };
                    return -1
                };

                let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
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
                    ctx.lower_polled(&write_polled);
                }
                ctx.raise_polled(&read_polled);

                amount_written as isize
            },
            FdResource::Stdin => 0,
            FdResource::Stdout => match &ctx.global.stdio {
                StdioBackend::Passthrough => unreachable!(),
                StdioBackend::Peered(_) => unreachable!(),
                StdioBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf.clone();
                    let write_polled = feedback.write_polled.clone();
                    let read_polled = feedback.read_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(&write_polled);
                    }
                    ctx.raise_polled(&read_polled);

                    written as isize
                },
                StdioBackend::Plugin(plugin_id) => {
                    let plugin_info = ctx.global.plugins.get(&plugin_id).unwrap();
                    let buffer_id = plugin_info.write_buf.clone();
                    let write_polled = plugin_info.write_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&write_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let written = buf.write(data);
                    if buf.is_full() {
                        ctx.lower_polled(&write_polled);
                    }

                    written as isize
                },
                StdioBackend::Sink => len as libc::ssize_t,
                StdioBackend::NullSink => len as libc::ssize_t,
                StdioBackend::Fuzz(_) => len as libc::ssize_t,
            },
            FdResource::Stderr => len as libc::ssize_t, // Transparently consume `stderr` output
            FdResource::Socket(socket_id) => match ctx.global.sockets.get(&socket_id).unwrap() {
                SocketState::Connected(_) => {
                    let socket_id = socket_id.clone();
                    drop(ctx);
                    send_connected_socket(&[data], socket_id, is_nonblocking)
                }
                SocketState::Connectionless(_) => {
                    let socket_id = socket_id.clone();
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

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);
        let data = slice::from_raw_parts(buf as *const u8, len);

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(&socket_id).unwrap() {
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

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        let data = slice::from_raw_parts(buf as *const u8, len);

        match ctx.global.sockets.get(&socket_id).unwrap() {
            SocketState::Connectionless(connectionless_sock) => {
                let addr = if dest_addr.is_null() {
                    None
                } else {
                    Some(match connectionless_sock.local_addr.family() {
                        AddressFamily::Ipv4 | AddressFamily::Ipv6 => match crate::decode_inet_address(dest_addr, addrlen) {
                            Ok(a) => TransportAddress::new_internet(a, connectionless_sock.local_addr.protocol()),
                            Err(_) => {
                                *libc::__errno_location() = libc::EINVAL;
                                return -1
                            }
                        }
                        AddressFamily::Unix => match crate::decode_unix_address(dest_addr, addrlen) {
                            Ok(a) => TransportAddress::Unix(a),
                            Err(_) => {
                                *libc::__errno_location() = libc::EINVAL;
                                return -1
                            }
                        }
                    })
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

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
            log::warn!("invalid file descriptor `{}` passed to `sendmsg`", fd);
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

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            log::warn!("non-socket file descriptor `{}` passed to `sendmsg`", fd);
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(&socket_id).unwrap() {
            SocketState::Connected(conn_info) => {
                if conn_info.peer_closed {
                    log::debug!("peer was closed--returning 0");
                    return 0
                }

                drop(ctx);
                send_connected_socket(&slices[..slice_cnt], socket_id, is_nonblocking)
            }
            SocketState::Connectionless(_) => {
                drop(ctx);
                todo!()
                //send_connectionless_socket(data, socket_id, is_nonblocking, None)
            }
            _ => {
                log::warn!("`sendmsg` called on unconnected socket {}", fd);
                *libc::__errno_location() = libc::ENOTCONN;
                return -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sendmmsg(
        _fd: libc::c_int,
        _msgvec: *mut libc::msghdr,
        _vlen: libc::c_uint,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_sendmmsg(_ctx) {

        if (flags & libc::MSG_FASTOPEN) != 0 {
            crate::report_strict_failure("fizzle does not currently implement TCP Fast Open")
        }

        panic!("`sendmsg` unimplemented");
    }
}

hook_macros::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_read(ctx) {

        log::debug!("read({}, buf length: {})", fd, len);

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
            log::warn!("read() called with unknown file descriptor");
            return hook_macros::real!(read)(fd, buf, len)
            /*
            *libc::__errno_location() = libc::EBADF;
            return -1
            */
        };

        let is_nonblocking = fd_info.nonblocking;
        let data = slice::from_raw_parts_mut(buf as *mut u8, len);

        match &fd_info.resource {
            FdResource::Epoll(_) | FdResource::Directory(_) => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            },
            FdResource::EventFd(eventfd_id) => {
                let eventfd_id = eventfd_id.clone();
                let Some(data) = data.get_mut(..8) else {
                    log::warn!("eventfd received `read` with invalid buffer length < 8");
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                };

                let eventfd = ctx.global.event_fds.get(&eventfd_id).unwrap();
                let is_semaphore = eventfd.is_semaphore;
                let old_counter = eventfd.counter;
                let read_polled = eventfd.read_polled.clone();
                let write_polled = eventfd.write_polled.clone();

                drop(ctx);

                if old_counter == 0 {
                    state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                }

                let mut ctx = state::FIZZLE_STATE.acquire();
                let eventfd = ctx.global.event_fds.get_mut(&eventfd_id).unwrap();

                let ret: u64 = match is_semaphore {
                    true => 1,
                    false => eventfd.counter,
                };

                if is_semaphore {
                    eventfd.counter -= 1;
                } else {
                    eventfd.counter = 0;
                }

                if eventfd.counter == 0 {
                    ctx.lower_polled(&read_polled);
                }
                ctx.raise_polled(&write_polled);

                data.copy_from_slice(ret.to_ne_bytes().as_slice());

                8
            }
            FdResource::File(file_id) => match ctx.global.files.get(&file_id).unwrap() {
                FileBackend::Passthrough => hook_macros::real!(read)(fd, buf, len),
                FileBackend::Peered(_) => unreachable!(),
                FileBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf.clone();
                    let read_polled = feedback.read_polled.clone();
                    let write_polled = feedback.write_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(&read_polled);
                    }
                    ctx.raise_polled(&write_polled);

                    read_len as isize
                },
                FileBackend::Plugin(plugin_id) => {
                    let plugin_id = plugin_id.clone();
                    let plugin_info = ctx.global.plugins.get(&plugin_id).unwrap();
                    let buffer_id = plugin_info.read_buf.clone();
                    let read_polled = plugin_info.read_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(&read_polled);
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
                FileBackend::Fuzz(fuzz_endpoint_id) => {
                    let fuzz_endpoint_id = fuzz_endpoint_id.clone();
                    let FuzzEndpointInfo { read_idx, read_polled } = ctx.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().clone();

                    let polled_is_ready = ctx.polled_is_ready(&read_polled);
                    drop(ctx);

                    if !polled_is_ready {
                        if is_nonblocking {
                            *libc::__errno_location() = libc::EAGAIN;
                            return -1
                        } else {

                            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let total_fuzz_len = ctx.global.fuzz_input.len();
                    let read_len = cmp::min(total_fuzz_len - read_idx, len);
                    data[..read_len].copy_from_slice(&ctx.global.fuzz_input.data()[read_idx..read_idx + read_len]);

                    let fuzz_endpoint = ctx.global.fuzz_endpoints.get_mut(&fuzz_endpoint_id).unwrap();
                    fuzz_endpoint.read_idx += read_len;
                    if fuzz_endpoint.read_idx == total_fuzz_len {
                        ctx.lower_polled(&read_polled);
                    }

                    read_len as libc::ssize_t
                },
            }
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => {
                let pipe_id = pipe_id.clone();
                let pipe_info = ctx.global.pipes.get(&pipe_id).unwrap();
                let peer_is_closed = pipe_info.peer.is_none();

                let buffer_id = pipe_info.read_buf.clone();
                let write_polled = pipe_info.write_polled.clone();
                let read_polled = pipe_info.read_polled.clone();

                let pipe_mode = pipe_info.mode;
                let polled_is_ready = ctx.polled_is_ready(&write_polled);
                drop(ctx);

                if !polled_is_ready {
                    if peer_is_closed {
                        return 0
                    } else if is_nonblocking {
                        unsafe { *libc::__errno_location() = libc::EAGAIN };
                        return -1
                    } else {
                        state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                    }
                }

                let mut ctx = state::FIZZLE_STATE.acquire();

                if ctx.global.pipes.get(&pipe_id).unwrap().peer.is_none() {
                    return 0
                }

                let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                let amount_read = match pipe_mode {
                    PipeMode::Direct => {
                        let mut packet_len_bytes = [0u8; 2];
                        assert!(buf.read(&mut packet_len_bytes) == 2);
                        buf.read(&mut data[..cmp::min(u16::from_be_bytes(packet_len_bytes) as usize, PIPE_BUF)])
                    },
                    PipeMode::Streamed => buf.read(data),
                };

                if buf.is_empty() {
                    ctx.lower_polled(&read_polled);
                }
                ctx.raise_polled(&write_polled);

                return amount_read as isize
            },
            FdResource::Stdin => match &ctx.global.stdio {
                StdioBackend::Passthrough => unreachable!(),
                StdioBackend::Peered(_) => unreachable!(),
                StdioBackend::Feedback(feedback) => {
                    let buffer_id = feedback.buf.clone();
                    let read_polled = feedback.read_polled.clone();
                    let write_polled = feedback.write_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(&read_polled);
                    }
                    ctx.raise_polled(&write_polled);

                    read_len as isize
                },
                StdioBackend::Plugin(plugin_id) => {
                    let plugin_info = ctx.global.plugins.get(&plugin_id).unwrap();
                    let buffer_id = plugin_info.write_buf.clone();
                    let read_polled = plugin_info.read_polled.clone();

                    let event_raised = ctx.global.polled_events.get(&read_polled).unwrap().event_raised;
                    drop(ctx);

                    if !event_raised {
                        if is_nonblocking {
                            unsafe { *libc::__errno_location() = libc::EAGAIN };
                            return -1
                        } else {
                            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
                    let read_len = buf.read(data);

                    if buf.is_empty() {
                        ctx.lower_polled(&read_polled);
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
                StdioBackend::Fuzz(fuzz_endpoint_id) => {
                    let fuzz_endpoint_id = fuzz_endpoint_id.clone();
                    let FuzzEndpointInfo { read_idx, read_polled } = ctx.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().clone();

                    let polled_is_ready = ctx.polled_is_ready(&read_polled);
                    drop(ctx);

                    if !polled_is_ready {
                        if is_nonblocking {
                            *libc::__errno_location() = libc::EAGAIN;
                            return -1
                        } else {

                            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                        }
                    }

                    let mut ctx = state::FIZZLE_STATE.acquire();

                    let total_fuzz_len = ctx.global.fuzz_input.len();
                    let read_len = cmp::min(ctx.global.fuzz_input.len() - read_idx, len);
                    data[..read_len].copy_from_slice(&ctx.global.fuzz_input.data()[read_idx..read_idx + read_len]);

                    let fuzz_endpoint = ctx.global.fuzz_endpoints.get_mut(&fuzz_endpoint_id).unwrap();
                    let read_polled = fuzz_endpoint.read_polled.clone();
                    fuzz_endpoint.read_idx += read_len;
                    if fuzz_endpoint.read_idx == total_fuzz_len {
                        ctx.lower_polled(&read_polled);
                    }

                    read_len as libc::ssize_t
                },
            },
            FdResource::Stdout => 0,
            FdResource::Stderr => 0,
            FdResource::Socket(socket_id) => match ctx.global.sockets.get(&socket_id).unwrap() {
                SocketState::Connected(_) => {
                    let socket_id = socket_id.clone();
                    drop(ctx);
                    recv_connected_socket(&mut [data], socket_id, is_nonblocking)
                }
                SocketState::Connectionless(_) => {
                    let socket_id = socket_id.clone();
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

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking || ((flags & libc::MSG_DONTWAIT) != 0);
        let data = slice::from_raw_parts_mut(buf as *mut u8, len);

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match ctx.global.sockets.get(&socket_id).unwrap() {
            SocketState::Connected(_) => {
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

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_camel_case_types)]
struct sctp_shutdown_event {
    spc_type: u16,
    spc_flags: u16,
    spc_length: u32,
    sse_assoc_id: libc::sctp_assoc_t,
}

const SCTP_SHUTDOWN_EVENT: u16 = (1 << 15) + 5;

hook_macros::hook! {
    unsafe fn recvmsg(
        fd: libc::c_int,
        msg: *mut libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recvmsg(ctx) {

        // TODO: have to fill out all the other fields in `msg`
        // For now we do this:
        (*msg).msg_controllen = 0;
        (*msg).msg_flags = libc::MSG_EOR;

        let Some(fd_info) = ctx.local.fds.get(&DescriptorId::new(fd)) else {
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

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        // Set CMSG headers
        let sndrcvinfo_len = libc::CMSG_LEN(mem::size_of::<libc::sctp_sndrcvinfo>() as u32) as usize;
        if (*msg).msg_controllen >= sndrcvinfo_len {
            // NOTE: this should only be for SCTP sockets...
            (*msg).msg_controllen = sndrcvinfo_len;
            let header = libc::cmsghdr {
                cmsg_len: mem::size_of::<libc::sctp_sndrcvinfo>(),
                cmsg_level: libc::IPPROTO_SCTP,
                cmsg_type: libc::SCTP_SNDRCV,
            };

            let sndrcvinfo = libc::sctp_sndrcvinfo {
                sinfo_stream: 0,
                sinfo_ssn: 0,
                sinfo_flags: 0,
                sinfo_ppid: 18, // TODO: make configurable
                sinfo_context: 0,
                sinfo_timetolive: 0,
                sinfo_tsn: 0,
                sinfo_cumtsn: 0,
                sinfo_assoc_id: 0,
            };

            let control_data = slice::from_raw_parts_mut((*msg).msg_control as *mut u8, (*msg).msg_controllen);
            let (header_data, sndrcvinfo_data) = control_data.split_at_mut(mem::size_of::<libc::sctp_sndrcvinfo>());
            header_data.copy_from_slice(slice::from_raw_parts(ptr::addr_of!(header) as *const u8, mem::size_of::<libc::cmsghdr>()));
            sndrcvinfo_data.copy_from_slice(slice::from_raw_parts(ptr::addr_of!(sndrcvinfo) as *const u8, mem::size_of::<libc::sctp_sndrcvinfo>()));
        }

        let recv_addr = (*msg).msg_name as *mut libc::sockaddr;

        match ctx.global.sockets.get(&socket_id).unwrap() {
            SocketState::Connected(conn_info) => {
                match &conn_info.rem_addr {
                    TransportAddress::Sctp(addr) | TransportAddress::Tcp(addr) | TransportAddress::Udp(addr) => {
                        let address_buf = slice::from_raw_parts_mut((*msg).msg_name as *mut u8, (*msg).msg_namelen as usize);
                        (*msg).msg_namelen = crate::encode_inet_address(address_buf, addr);
                    },
                    TransportAddress::Unix(addr) => crate::encode_unix_address(recv_addr, ptr::addr_of_mut!((*msg).msg_namelen), addr),
                }

                if conn_info.peer_closed && conn_info.rem_addr.protocol() == TransportProtocol::Sctp {
                    // TODO: support vslices
                    assert!(slices.len() == 1 || slices[0].len() >= mem::size_of::<sctp_shutdown_event>(), "vectored I/O unsupported for closed recvmsg");

                    (*msg).msg_flags = libc::MSG_NOTIFICATION;

                    let shutdown = sctp_shutdown_event {
                        spc_type: SCTP_SHUTDOWN_EVENT,
                        spc_flags: 0,
                        spc_length: mem::size_of::<sctp_shutdown_event>() as u32,
                        sse_assoc_id: 0,
                    };

                    let shutdown_slice = unsafe { slice::from_raw_parts(ptr::addr_of!(shutdown) as *const u8, mem::size_of_val(&shutdown)) };

                    slices[0][..shutdown_slice.len()].copy_from_slice(shutdown_slice);

                    return shutdown_slice.len() as libc::ssize_t
                }

                drop(ctx);
                recv_connected_socket(&mut slices[..slice_cnt], socket_id, is_nonblocking)
            },
            SocketState::Connectionless(_) => {
                // decode who the message was received from and put it in here:
                /*
                (*msg).msg_namelen = match &conn_info.rem_addr {
                    TransportAddress::Sctp(addr) | TransportAddress::Tcp(addr) | TransportAddress::Udp(addr) => crate::encode_inet_address(recv_addr, addr),
                    TransportAddress::Unix(addr) => crate::encode_unix_address(recv_addr, addr),
                } as u32;
                */
                drop(ctx);
                // TODO: fix this!! Doesn't accurately handle iovec
                recv_connectionless_socket(slices[0], socket_id.clone(), is_nonblocking, recv_addr, ptr::addr_of_mut!((*msg).msg_namelen))
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
    addr: &TransportAddress,
) -> libc::ssize_t {
    let mut sockaddr: MaybeUninit<libc::sockaddr_storage> = MaybeUninit::uninit();
    let mut addrlen = mem::size_of::<libc::sockaddr_storage>() as u32;
    match addr {
        TransportAddress::Udp(socket_addr) => {
            let address_buf = unsafe { slice::from_raw_parts_mut(ptr::addr_of_mut!(sockaddr) as *mut u8, addrlen as usize) };
            crate::encode_inet_address(address_buf, socket_addr);
        }
        TransportAddress::Unix(unix_addr) =>
            crate::encode_unix_address(sockaddr.as_mut_ptr() as *mut libc::sockaddr, ptr::addr_of_mut!(addrlen), unix_addr),
        _ => unreachable!(),
    };

    let sockaddr_bytes = unsafe { slice::from_raw_parts(sockaddr.as_ptr() as *const u8, addrlen as usize) };

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
    socket_id: Rc<SocketId>,
    is_nonblocking: bool,
) -> libc::ssize_t {
    let mut ctx = state::FIZZLE_STATE.acquire();

    let SocketState::Connected(ConnectedSocket { backend, peer_closed, .. }) =
        ctx.global.sockets.get(&socket_id).unwrap()
    else {
        unreachable!()
    };

    // TODO: potentially make this more DRY
    match backend {
        ConnectedBackend::Passthrough => unimplemented!(),
        ConnectedBackend::Peered(regular) => {
            let Some(peer) = regular.peer.clone() else {
                log::debug!("connected peer was closed during attempted socket send");
                return 0; // No more information to write to the connected socket
            };

            if *peer_closed {
                return 0
            }

            let Some(SocketState::Connected(ConnectedSocket {
                backend: IoBackend::Peered(regular_peer),
                ..
            })) = ctx.global.sockets.get(&peer)
            else {
                unreachable!()
            };

            let buffer_id = regular_peer.recv_buf.clone();
            let write_polled = regular_peer.write_polled.clone();
            let read_polled = regular_peer.read_polled.clone();

            let polled_is_ready = ctx.polled_is_ready(&write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    log::debug!("nonblocking socket not ready for send--returning EAGAIN");
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket {
                backend: IoBackend::Peered(RegularConnected { peer: Some(_), .. }),
                ..
            })) = ctx.global.sockets.get(&socket_id)
            else {
                return 0;
            };

            let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let mut total_written = 0;
            for slice in data.iter() {
                let written = buf.write(slice);
                if written > 0 {
                    total_written += written;
                }

                if written != slice.len() {
                    break;
                }
            }

            if buf.is_full() {
                ctx.lower_polled(&write_polled);
            }
            ctx.raise_polled(&read_polled);

            total_written as isize
        }
        ConnectedBackend::Feedback(feedback) => {
            let buffer_id = feedback.buf.clone();
            let write_polled = feedback.write_polled.clone();
            let read_polled = feedback.read_polled.clone();

            let polled_is_ready = ctx.polled_is_ready(&write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket {
                backend: IoBackend::Peered(RegularConnected { peer: Some(_), .. }),
                ..
            })) = ctx.global.sockets.get(&socket_id)
            else {
                return 0;
            };

            let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let mut total_written = 0;
            for slice in data.iter() {
                let written = buf.write(slice);
                if written > 0 {
                    total_written += written;
                }

                if written != slice.len() {
                    break;
                }
            }

            if buf.is_full() {
                ctx.lower_polled(&write_polled);
            }
            ctx.raise_polled(&read_polled);

            total_written as isize
        }
        ConnectedBackend::Plugin(plugin_id) => {
            let plugin_info = ctx.global.plugins.get(plugin_id).unwrap();
            let buffer_id = plugin_info.write_buf.clone();
            let write_polled = plugin_info.write_polled.clone();

            let polled_is_ready = ctx.polled_is_ready(&write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We need to verify that this connection has not shut down before writing to the same buffer_id
            let Some(SocketState::Connected(ConnectedSocket {
                backend: IoBackend::Peered(RegularConnected { peer: Some(_), .. }),
                ..
            })) = ctx.global.sockets.get(&socket_id)
            else {
                return 0;
            };

            let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let mut total_written = 0;
            for slice in data.iter() {
                let written = buf.write(slice);
                if written > 0 {
                    total_written += written;
                }

                if written != slice.len() {
                    break;
                }
            }

            if buf.is_full() {
                ctx.lower_polled(&write_polled);
            }

            total_written as isize
        }
        ConnectedBackend::Sink => data.iter().map(|s| s.len()).sum::<usize>() as libc::ssize_t,
        ConnectedBackend::NullSink => data.iter().map(|s| s.len()).sum::<usize>() as libc::ssize_t,
        ConnectedBackend::Fuzz(_) => data.iter().map(|s| s.len()).sum::<usize>() as libc::ssize_t,
    }
}

const MAX_DATAGRAM: usize = 65507;
const MAX_INTERNAL_DATAGRAM: usize = MAX_DATAGRAM + mem::size_of::<libc::sockaddr_storage>() + 3;

fn send_connectionless_socket(
    data: &[u8],
    socket_id: Rc<SocketId>,
    is_nonblocking: bool,
    addr: Option<TransportAddress>,
) -> libc::ssize_t {
    let mut ctx = state::FIZZLE_STATE.acquire();

    let SocketState::Connectionless(sock_info) = ctx.global.sockets.get(&socket_id).unwrap() else {
        unreachable!()
    };

    if data.len() > MAX_DATAGRAM {
        unsafe { *libc::__errno_location() = libc::EMSGSIZE };
        return -1;
    }

    let Some(rem_addr) = addr.or(sock_info.rem_addr.clone()) else {
        unsafe { *libc::__errno_location() = libc::ENOTCONN };
        return -1;
    };

    let local_addr = sock_info.local_addr.clone();

    let Some(SocketLocationInfo {
        bound_socket: Some(peer_sock_id),
        ..
    }) = ctx
        .global
        .socket_locations
        .get(&rem_addr)
    else {
        unsafe { *libc::__errno_location() = libc::ECONNRESET }; // No socket was listening at the endpoint
        return -1; // TODO: should we just return data.len() here instead?
    };
    
    let SocketState::Connectionless(peer_info) = ctx.global.sockets.get(&peer_sock_id).unwrap()
    else {
        unreachable!()
    };

    match &peer_info.backend {
        ConnectionlessBackend::Passthrough => unimplemented!(),
        ConnectionlessBackend::Peered(regular_peer) => {
            let write_polled = regular_peer.write_polled.clone();

            let polled_is_ready = ctx.polled_is_ready(&write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
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
                .get(&rem_addr)
            else {
                return data.len() as libc::ssize_t; // Drop packet
            };

            let SocketState::Connectionless(ConnectionlessSocket {
                backend: IoBackend::Peered(regular_peer),
                ..
            }) = ctx.global.sockets.get(&peer_sock_id).unwrap()
            else {
                return data.len() as libc::ssize_t; // Drop packet
            };

            let buffer_id = regular_peer.recv_buf.clone();
            let write_polled = regular_peer.write_polled.clone();
            let read_polled = regular_peer.read_polled.clone();

            // Re-doing all this accounts for a nasty (though unlikely) TOCTOU bug that could show up if the
            // destination UDP server disconnects and another takes it place while this thread is polling.
            if !ctx.polled_is_ready(&write_polled) {
                return data.len() as libc::ssize_t; // Drop packet
            }

            let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &rem_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(&write_polled);
            }
            ctx.raise_polled(&read_polled);

            amount_written as isize
        }
        ConnectionlessBackend::Feedback(feedback) => {
            let buffer_id = feedback.buf.clone();
            let write_polled = feedback.write_polled.clone();
            let read_polled = feedback.read_polled.clone();

            let polled_is_ready = ctx.polled_is_ready(&write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We don't need to verify that this connection has not shut down, as it's a Feedback endpoint

            let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &local_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(&write_polled);
            }
            ctx.raise_polled(&read_polled);

            amount_written as isize
        }
        ConnectionlessBackend::Plugin(plugin_id) => {
            let plugin_info = ctx.global.plugins.get(&plugin_id).unwrap();
            let buffer_id = plugin_info.write_buf.clone();
            let write_polled = plugin_info.write_polled.clone();

            let polled_is_ready = ctx.polled_is_ready(&write_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            // We don't need to verify that this connection has not shut down, as it's a Plugin endpoint

            let buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let amount_written = write_datagram(buf, data, &rem_addr);

            if FIZZLE_BUFFER_LENGTH - buf.len() < MAX_INTERNAL_DATAGRAM {
                ctx.lower_polled(&write_polled);
            }

            amount_written as isize
        }
        ConnectionlessBackend::Sink => data.len() as libc::ssize_t,
        ConnectionlessBackend::NullSink => data.len() as libc::ssize_t,
        ConnectionlessBackend::Fuzz(_) => data.len() as libc::ssize_t,
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
    socket_id: Rc<SocketId>,
    is_nonblocking: bool,
) -> libc::ssize_t {
    let mut ctx = state::FIZZLE_STATE.acquire();

    let SocketState::Connected(sock_info) = ctx.global.sockets.get(&socket_id).unwrap() else {
        panic!("internal error")
    };

    match &sock_info.backend {
        IoBackend::Passthrough => unimplemented!(),
        IoBackend::Peered(regular) => {
            let buf_id = regular.recv_buf.clone();
            let write_polled = regular.write_polled.clone();
            let read_polled = regular.read_polled.clone();
            let peer_is_shutdown = regular.peer.is_none() || sock_info.peer_closed;

            // First, check to see if we can just immediately read despite the peer being closed
            if ctx.polled_is_ready(&read_polled) {
                let recv_buf = ctx.global.buffers.get_mut(&buf_id).unwrap();
                let mut total_read = 0;
                for slice in data.iter_mut() {
                    let read_amount = recv_buf.read(slice);
                    if read_amount > 0 {
                        total_read += read_amount;
                    }

                    if read_amount != slice.len() {
                        break;
                    }
                }

                if recv_buf.is_empty() {
                    ctx.lower_polled(&read_polled);
                }
                ctx.raise_polled(&write_polled);

                return total_read as libc::ssize_t;
            }

            if peer_is_shutdown {
                return 0; // No more information to receive from the connected socket
            };

            if is_nonblocking {
                unsafe { *libc::__errno_location() = libc::EAGAIN };
                return -1;
            }

            // Our peer is still connected, and we're in blocking mode
            drop(ctx);
            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
            let mut ctx = state::FIZZLE_STATE.acquire();

            let recv_buf = ctx.global.buffers.get_mut(&buf_id).unwrap();
            let mut total_read = 0;
            for slice in data.iter_mut() {
                let read_amount = recv_buf.read(slice);
                if read_amount > 0 {
                    total_read += read_amount;
                }

                if read_amount != slice.len() {
                    break;
                }
            }

            if recv_buf.is_empty() {
                ctx.lower_polled(&read_polled);
            }
            ctx.raise_polled(&write_polled);

            total_read as libc::ssize_t
        }
        IoBackend::Feedback(feedback_info) => {
            let buf_id = feedback_info.buf.clone();
            let write_polled = feedback_info.write_polled.clone();
            let read_polled = feedback_info.read_polled.clone();

            // First, check to see if we can just immediately read despite the peer being closed
            if ctx.polled_is_ready(&read_polled) {
                let recv_buf = ctx.global.buffers.get_mut(&buf_id).unwrap();
                let mut total_read = 0;
                for slice in data.iter_mut() {
                    let read_amount = recv_buf.read(slice);
                    if read_amount > 0 {
                        total_read += read_amount;
                    }

                    if read_amount != slice.len() {
                        break;
                    }
                }

                if recv_buf.is_empty() {
                    ctx.lower_polled(&read_polled);
                }
                ctx.raise_polled(&write_polled);

                return total_read as libc::ssize_t;
            }

            if is_nonblocking {
                unsafe { *libc::__errno_location() = libc::EAGAIN };
                return -1;
            }

            // Our peer is still connected, and we're in blocking mode
            drop(ctx);
            state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
            let mut ctx = state::FIZZLE_STATE.acquire();

            let recv_buf = ctx.global.buffers.get_mut(&buf_id).unwrap();
            let mut total_read = 0;
            for slice in data.iter_mut() {
                let read_amount = recv_buf.read(slice);
                if read_amount > 0 {
                    total_read += read_amount;
                }

                if read_amount != slice.len() {
                    break;
                }
            }

            if recv_buf.is_empty() {
                ctx.lower_polled(&read_polled);
            }
            ctx.raise_polled(&write_polled);

            total_read as libc::ssize_t
        }
        IoBackend::Plugin(plugin_id) => {
            let plugin_info = ctx.global.plugins.get(&plugin_id).unwrap();
            let buffer_id = plugin_info.read_buf.clone();
            let read_polled = plugin_info.read_polled.clone();

            let event_raised = ctx
                .global
                .polled_events
                .get(&read_polled)
                .unwrap()
                .event_raised;
            drop(ctx);

            if !event_raised {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(read_polled.clone());
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            let recv_buf = ctx.global.buffers.get_mut(&buffer_id).unwrap();
            let mut total_read = 0;
            for slice in data.iter_mut() {
                let read_amount = recv_buf.read(slice);
                if read_amount > 0 {
                    total_read += read_amount;
                }

                if read_amount != slice.len() {
                    break;
                }
            }

            if recv_buf.is_empty() {
                ctx.lower_polled(&read_polled);
            }

            total_read as libc::ssize_t
        }
        IoBackend::Sink => 0 as libc::ssize_t,
        IoBackend::NullSink => {
            let mut total_len = 0;
            for slice in data.iter_mut() {
                for b in slice.iter_mut() {
                    *b = 0;
                }
                total_len += slice.len();
            }

            total_len as libc::ssize_t
        }
        IoBackend::Fuzz(fuzz_endpoint_id) => {
            let fuzz_endpoint_id = fuzz_endpoint_id.clone();
            let FuzzEndpointInfo {
                mut read_idx,
                read_polled,
            } = ctx.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().clone();

            let polled_is_ready = ctx.polled_is_ready(&read_polled);
            drop(ctx);

            if !polled_is_ready {
                if is_nonblocking {
                    unsafe { *libc::__errno_location() = libc::EAGAIN };
                    return -1;
                } else {
                    state::FIZZLE_STATE.poll_until_ready(read_polled);
                }
            }

            let mut ctx = state::FIZZLE_STATE.acquire();

            let total_fuzz_len = ctx.global.fuzz_input.len();
            let start_read_idx = read_idx;

            for slice in data.iter_mut() {
                let read_len = cmp::min(total_fuzz_len - read_idx, slice.len());
                slice[..read_len]
                    .copy_from_slice(&ctx.global.fuzz_input.data()[read_idx..read_idx + read_len]);
                read_idx += read_len;
                if read_idx == total_fuzz_len {
                    break;
                }
            }

            let fuzz_endpoint = ctx.global.fuzz_endpoints.get_mut(&fuzz_endpoint_id).unwrap();
            let read_polled = fuzz_endpoint.read_polled.clone();
            fuzz_endpoint.read_idx = read_idx;
            if fuzz_endpoint.read_idx == total_fuzz_len {
                ctx.lower_polled(&read_polled);
            }

            (read_idx - start_read_idx) as libc::ssize_t
        }
    }
}

fn recv_connectionless_socket(
    data: &mut [u8],
    socket_id: Rc<SocketId>,
    is_nonblocking: bool,
    addr: *mut libc::sockaddr,
    addrlen: *mut libc::socklen_t,
) -> libc::ssize_t {
    let mut ctx = state::FIZZLE_STATE.acquire();

    // TODO: this isn't finished...
    let SocketState::Connectionless(ConnectionlessSocket {
        backend: IoBackend::Peered(regular),
        ..
    }) = ctx.global.sockets.get(&socket_id).unwrap()
    else {
        unreachable!()
    };

    let buf_id = regular.recv_buf.clone();
    let write_polled = regular.write_polled.clone();
    let read_polled = regular.read_polled.clone();

    let polled_is_ready = ctx.polled_is_ready(&write_polled);
    drop(ctx);

    if !polled_is_ready {
        if is_nonblocking {
            unsafe { *libc::__errno_location() = libc::EAGAIN };
            return -1;
        } else {
            state::FIZZLE_STATE.poll_until_ready(write_polled.clone());
        }
    }

    let mut ctx = state::FIZZLE_STATE.acquire();

    let recv_buf = ctx.global.buffers.get_mut(&buf_id).unwrap();
    let read_len = read_datagram(recv_buf, data, addr, addrlen);

    if recv_buf.is_empty() {
        ctx.lower_polled(&read_polled);
    }
    ctx.raise_polled(&write_polled);

    read_len
}
