use crate::arena::{ArenaKey, Rc};
use crate::backend::{
    ConnectedBackend, ConnectingBackend, ConnectionlessBackend, PendingBackend, RegularConnected,
    RegularConnectionless, ServerBackend, StandardFeedback,
};
use crate::constants::{
    FIZZLE_BUFFER_LENGTH, FIZZLE_EPHEMERAL_PORT_END, FIZZLE_MAX_REUSEPORT,
    FIZZLE_MIN_CONNECTIONLESS, FIZZLE_SOMAXCONN,
};
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::{FizzleSingleton, FizzleState};
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::time::Duration;
use std::{cmp, mem, slice};

use fizzle_common::io::{AddressFamily, SockAddr, SocketType, TransportAddress, TransportProtocol};
use fizzle_common::storage::Buffer;
use heapless::{Deque, Entry};
pub use private::SocketId;

use super::buffer::BufferId;
use super::descriptor::{DescriptorError, DescriptorId, DescriptorInfo, FdResource};
use super::fuzz_endpoint::FuzzEndpointInfo;
use super::polled::{PolledId, PolledInfo};
use super::poller::PollerId;
use super::{init_from_slice, FfiOutput, MsgHdr, MsgHdrOut};

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SocketId(usize);
}

#[derive(Debug)]
pub struct TransportLocationInfo {
    pub reuse_port: bool,
    pub bound_sockets: Deque<Rc<SocketId>, FIZZLE_MAX_REUSEPORT>,
    pub pending: Option<PendingInfo>,
}

#[derive(Clone, Debug)]
pub struct PendingInfo {
    pub client: Rc<SocketId>,
    pub poll: Rc<PolledId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalAddress {
    Ephemeral(AddressFamily),
    Assigned(SockAddr),
}

impl LocalAddress {
    pub fn family(&self) -> AddressFamily {
        match self {
            LocalAddress::Ephemeral(address_family) => *address_family,
            LocalAddress::Assigned(sock_addr) => sock_addr.family(),
        }
    }
}

#[derive(Debug)]
pub struct SocketInfo {
    /// The number of file descriptors currently referencing the socket.
    pub fd_count: usize,
    pub socktype: SocketType,
    pub protocol: TransportProtocol,
    // NOTE: assigning ephemeral address at socket creation will exhaust available ephemeral
    // addresses rather quickly given our current scheme. If this becomes an issue, consider
    // revising this bit of code.
    /// The local address the socket is bound to.
    ///
    /// By default, this is an ephemeral address assigned at socket creation.
    pub local_addr: LocalAddress,
    pub state: SocketState,
}

impl SocketInfo {
    pub fn new_unassociated(
        family: AddressFamily,
        socktype: SocketType,
        protocol: TransportProtocol,
    ) -> Self {
        Self {
            fd_count: 1,
            socktype,
            protocol,
            local_addr: LocalAddress::Ephemeral(family),
            state: SocketState::Unassociated(UnassociatedSocket { reuse_port: false }),
        }
    }

    pub fn local_transport(&self) -> Option<TransportAddress> {
        match self.local_addr.clone() {
            LocalAddress::Assigned(sockaddr) => Some(TransportAddress {
                sockaddr,
                protocol: self.protocol,
            }),
            LocalAddress::Ephemeral(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum SocketState {
    Connectionless(ConnectionlessSocket),
    Unassociated(UnassociatedSocket),
    Server(ServerSocket),
    PendingConnection(PendingSocket),
    Connecting(ConnectingSocket),
    Connected(ConnectedSocket),
}

#[derive(Debug)]
pub struct ConnectionlessSocket {
    pub backend: ConnectionlessBackend,
    pub rem_addr: Option<TransportAddress>,
    pub reuse_port: bool,
}

#[derive(Debug)]
pub struct UnassociatedSocket {
    reuse_port: bool,
}

#[derive(Debug)]
pub struct ServerSocket {
    pub backend: ServerBackend,
    pub connecting: heapless::Deque<Rc<SocketId>, FIZZLE_SOMAXCONN>,
    pub ready_to_connect: Rc<PolledId>,
}

#[derive(Clone, Debug)]
pub struct PendingSocket {
    pub backend: PendingBackend,
    pub next_pending: Option<Rc<SocketId>>,
    pub rem_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectingSocket {
    pub backend: ConnectingBackend,
    pub connect_polled: Rc<PolledId>,
}

#[derive(Debug)]
pub struct ConnectedSocket {
    pub backend: ConnectedBackend,
    pub rem_addr: TransportAddress,
    pub peer_closed: bool,
}

impl ArenaKey for SocketId {
    type Value = SocketInfo;
}

impl Rc<SocketId> {
    pub fn read(
        &self,
        ctx: &mut FizzleSingleton,
        msg: &mut MsgHdrOut,
        nonblocking: bool,
    ) -> Result<usize, SocketError> {
        let mut state = ctx.acquire();

        match state.global.sockets.get(self).unwrap() {
            SocketState::Connectionless(conn) => {
                let read_polled: Option<Rc<PolledId>>;
                let write_polled: Rc<PolledId>;
                let buffer_id: Rc<BufferId>;

                match &conn.backend {
                    ConnectionlessBackend::Peered(regular) => {
                        read_polled = Some(regular.read_polled.clone());
                        write_polled = regular.write_polled.clone();
                        buffer_id = regular.recv_buf.clone();
                    }
                    /*
                    ConnectionlessBackend::Feedback(feedback) => {
                        read_polled = Some(feedback.read_polled.clone());
                        write_polled = feedback.write_polled.clone();
                        buffer_id = feedback.buf.clone();
                    }
                    ConnectionlessBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(plugin_id).unwrap();

                        read_polled = None;
                        write_polled = plugin_info.write_polled.clone();
                        buffer_id = plugin_info.write_buf.clone();
                    }
                    ConnectionlessBackend::Sink => return Ok(0),
                    ConnectionlessBackend::NullSink => {
                        let mut total_read = 0;
                        for iovec in msg.vdata_mut() {
                            for b in iovec.data_mut() {
                                b.write(0);
                            }
                            total_read += iovec.data_mut().len();
                        }

                        return Ok(total_read)
                    }
                    ConnectionlessBackend::Fuzz(fuzz_endpoint_id) => {
                        let fuzz_endpoint_id = fuzz_endpoint_id.clone();
                        let FuzzEndpointInfo { mut read_idx, read_polled } = state.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            ctx.poll_until_ready(read_polled.clone());
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.fuzz_input.data();
                        let buflen = buf.len();

                        let mut total_read = 0;
                        for iovec in msg.vdata_mut() {
                            if buf[read_idx..].is_empty() {
                                break
                            }

                            let data_len = cmp::min(buf.len(), iovec.data_mut().len());
                            init_from_slice(&mut iovec.data_mut()[..data_len], &buf[read_idx..read_idx + data_len]);
                            read_idx += data_len;
                            total_read += data_len;
                        }

                        let fuzz_endpoint = state.global.fuzz_endpoints.get_mut(&fuzz_endpoint_id).unwrap();
                        fuzz_endpoint.read_idx = read_idx;
                        if fuzz_endpoint.read_idx == buflen {
                            state.lower_polled(&read_polled);
                        }

                        // special case here--we need to make up endpoint info to be fuzzing from.
                        msg.set_ancillary_len(0);
                        let addr_bytes = msg.addr_bytes();


                        return Ok(total_read)
                    },
                    */
                    _ => unreachable!(),
                }

                if let Some(read_polled) = read_polled.as_ref() {
                    let polled_is_ready = state.polled_is_ready(&read_polled);
                    drop(state);

                    if !polled_is_ready {
                        if nonblocking {
                            return Err(SocketError::WouldBlock);
                        } else {
                            ctx.poll_until_ready(write_polled.clone());
                        }
                    }
                } else {
                    drop(state);
                }

                let mut state = ctx.acquire();

                let read_buffer = state.global.buffers.get_mut(&buffer_id).unwrap();
                let total_read = super::read_datagram(msg, read_buffer).unwrap();

                let available = read_buffer.write_available();
                let buffer_empty = read_buffer.is_empty();

                if available >= FIZZLE_MIN_CONNECTIONLESS {
                    state.raise_polled(&write_polled);
                }

                if buffer_empty {
                    if let Some(read_polled) = read_polled {
                        state.lower_polled(&read_polled);
                    }
                }

                Ok(total_read)
            }
            SocketState::Connected(conn) => {
                let read_polled: Rc<PolledId>;
                let write_polled: Rc<PolledId>;
                let buffer_id: Rc<BufferId>;

                let peer_closed = conn.peer_closed;

                let rem_sockaddr = conn.rem_addr.addr().clone();

                match &conn.backend {
                    ConnectedBackend::Passthrough => unreachable!(),
                    ConnectedBackend::Peered(regular) => {
                        read_polled = regular.read_polled.clone();
                        write_polled = regular.write_polled.clone();
                        buffer_id = regular.recv_buf.clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            if peer_closed {
                                return Ok(0);
                            }
                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0); // The connection was shut down while we were polling on it
                        };

                        drop(state);
                    }
                    ConnectedBackend::Feedback(feedback) => {
                        read_polled = feedback.read_polled.clone();
                        write_polled = feedback.write_polled.clone();
                        buffer_id = feedback.buf.clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            if peer_closed {
                                return Ok(0);
                            }
                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        // We need to verify that this connection has not shut down before writing to the same buffer_id
                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0);
                        };

                        drop(state);
                    }
                    ConnectedBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(plugin_id).unwrap();

                        read_polled = plugin_info.read_polled.clone();
                        write_polled = plugin_info.write_polled.clone();
                        buffer_id = plugin_info.write_buf.clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            if peer_closed {
                                return Ok(0);
                            }
                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        // We need to verify that this connection has not shut down before writing to the same buffer_id
                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0);
                        };

                        drop(state);
                    }
                    ConnectedBackend::Sink => return Ok(0),
                    ConnectedBackend::NullSink => {
                        let mut total_read = 0;
                        for iovec in msg.vdata_mut() {
                            for b in iovec.data_mut() {
                                b.write(0);
                            }
                            total_read += iovec.data_mut().len();
                        }

                        return Ok(total_read);
                    }
                    ConnectedBackend::Fuzz(fuzz_endpoint_id) => {
                        let fuzz_endpoint_id = fuzz_endpoint_id.clone();
                        let FuzzEndpointInfo {
                            mut read_idx,
                            read_polled,
                        } = state
                            .global
                            .fuzz_endpoints
                            .get(&fuzz_endpoint_id)
                            .unwrap()
                            .clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            if peer_closed {
                                return Ok(0);
                            }

                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(read_polled.clone());
                            }
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.fuzz_input.data();
                        let buflen = buf.len();

                        let mut total_read = 0;
                        for iovec in msg.vdata_mut() {
                            if buf[read_idx..].is_empty() {
                                break;
                            }

                            let data_len = cmp::min(buf.len(), iovec.data_mut().len());
                            init_from_slice(
                                &mut iovec.data_mut()[..data_len],
                                &buf[read_idx..read_idx + data_len],
                            );
                            read_idx += data_len;
                            total_read += data_len;
                        }

                        let fuzz_endpoint = state
                            .global
                            .fuzz_endpoints
                            .get_mut(&fuzz_endpoint_id)
                            .unwrap();
                        fuzz_endpoint.read_idx = read_idx;
                        if fuzz_endpoint.read_idx == buflen {
                            state.lower_polled(&read_polled);
                        }

                        msg.set_ancillary_len(0);
                        let addrlen = rem_sockaddr.encode(msg.addr_bytes()) as u32;
                        msg.set_addrlen(addrlen);

                        return Ok(total_read);
                    }
                }

                let mut state = ctx.acquire();

                let read_buffer = state.global.buffers.get_mut(&buffer_id).unwrap();
                let total_read = super::read_stream(msg, read_buffer.data());
                read_buffer.did_read(total_read);

                if read_buffer.is_empty() {
                    state.lower_polled(&read_polled);
                }

                state.raise_polled(&write_polled);

                Ok(total_read)
            }
            _ => Err(SocketError::InvalidState),
        }
    }

    pub fn write(
        &self,
        ctx: &mut FizzleSingleton,
        msg: &impl MsgHdr,
        nonblocking: bool,
    ) -> Result<usize, SocketError> {
        let mut state = ctx.acquire();

        match state.global.sockets.get(self).unwrap() {
            SocketState::Connectionless(conn) => {
                let read_polled: Option<Rc<PolledId>>;
                let write_polled: Rc<PolledId>;
                let buffer_id: Rc<BufferId>;

                match &conn.backend {
                    ConnectionlessBackend::Passthrough => unreachable!(),
                    ConnectionlessBackend::Peered(_regular) => {
                        let dst_addr = TransportAddress {
                            sockaddr: msg.addr().map_err(|_| SocketError::InvalidAddress)?,
                            protocol: conn.local_addr.protocol(),
                        };

                        let socket_id: Rc<SocketId>;

                        if let Some(rem_socket_id) = state
                            .global
                            .socket_locations
                            .get_mut(&dst_addr)
                            .and_then(|i| i.bound_sockets.front().cloned())
                        {
                            socket_id = rem_socket_id;
                        } else if let Some(rem_socket_id) = Self::wildcard_addr(&dst_addr)
                            .and_then(|a| state.global.socket_locations.get_mut(&a))
                            .and_then(|i| i.bound_sockets.front().cloned())
                        {
                            socket_id = rem_socket_id;
                        } else {
                            log::error!("packet send to location {} that had no listening ports--packet silently dropped", dst_addr);
                            return Ok(msg.vdata().iter().map(|v| v.data().len()).sum());
                        }

                        let SocketState::Connectionless(conn) =
                            state.global.sockets.get(&socket_id).unwrap()
                        else {
                            unreachable!()
                        };

                        let ConnectionlessBackend::Peered(peer) = &conn.backend else {
                            unreachable!()
                        };

                        read_polled = Some(peer.read_polled.clone());
                        write_polled = peer.write_polled.clone();
                        buffer_id = peer.recv_buf.clone();
                    }
                    ConnectionlessBackend::Feedback(feedback) => {
                        read_polled = Some(feedback.read_polled.clone());
                        write_polled = feedback.write_polled.clone();
                        buffer_id = feedback.buf.clone();
                    }
                    ConnectionlessBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(plugin_id).unwrap();

                        read_polled = None;
                        write_polled = plugin_info.write_polled.clone();
                        buffer_id = plugin_info.write_buf.clone();
                    }
                    ConnectionlessBackend::Sink
                    | ConnectionlessBackend::NullSink
                    | ConnectionlessBackend::Fuzz(_) => {
                        return Ok(msg.vdata().iter().map(|v| v.data().len()).sum())
                    }
                }

                let polled_is_ready = state.polled_is_ready(&write_polled);
                drop(state);

                if !polled_is_ready {
                    if nonblocking {
                        return Err(SocketError::WouldBlock);
                    } else {
                        ctx.poll_until_ready(write_polled.clone());
                    }
                }

                let mut state = ctx.acquire();

                let write_buffer = state.global.buffers.get_mut(&buffer_id).unwrap();
                let total_written = super::write_datagram(msg, write_buffer).unwrap();

                let available = write_buffer.write_available();

                if available < FIZZLE_MIN_CONNECTIONLESS {
                    state.lower_polled(&write_polled);
                }

                if let Some(read_polled) = read_polled {
                    state.raise_polled(&read_polled);
                }

                Ok(total_written)
            }
            SocketState::Connected(conn) => {
                if conn.peer_closed {
                    return Ok(0);
                }

                let read_polled: Option<Rc<PolledId>>;
                let write_polled: Rc<PolledId>;
                let buffer_id: Rc<BufferId>;

                match &conn.backend {
                    ConnectedBackend::Passthrough => unreachable!(),
                    ConnectedBackend::Peered(regular) => {
                        let Some(peer) = regular.peer.clone() else {
                            unreachable!()
                        };

                        let Some(SocketState::Connected(ConnectedSocket {
                            backend: ConnectedBackend::Peered(regular_peer),
                            ..
                        })) = state.global.sockets.get(&peer)
                        else {
                            unreachable!()
                        };

                        read_polled = Some(regular_peer.read_polled.clone());
                        write_polled = regular_peer.write_polled.clone();
                        buffer_id = regular_peer.recv_buf.clone();

                        let polled_is_ready = state.polled_is_ready(&write_polled);
                        drop(state);

                        if !polled_is_ready {
                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            backend:
                                ConnectedBackend::Peered(RegularConnected { peer: Some(_), .. }),
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0); // The connection was shut down while we were polling on it
                        };

                        drop(state);
                    }
                    ConnectedBackend::Feedback(feedback) => {
                        read_polled = Some(feedback.read_polled.clone());
                        write_polled = feedback.write_polled.clone();
                        buffer_id = feedback.buf.clone();

                        let polled_is_ready = state.polled_is_ready(&write_polled);
                        drop(state);

                        if !polled_is_ready {
                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        // We need to verify that this connection has not shut down before writing to the same buffer_id
                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0);
                        };

                        drop(state);
                    }
                    ConnectedBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(plugin_id).unwrap();

                        read_polled = None;
                        write_polled = plugin_info.write_polled.clone();
                        buffer_id = plugin_info.write_buf.clone();

                        let polled_is_ready = state.polled_is_ready(&write_polled);
                        drop(state);

                        if !polled_is_ready {
                            if nonblocking {
                                return Err(SocketError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        // We need to verify that this connection has not shut down before writing to the same buffer_id
                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0);
                        };

                        drop(state);
                    }
                    ConnectedBackend::Sink
                    | ConnectedBackend::NullSink
                    | ConnectedBackend::Fuzz(_) => {
                        return Ok(msg.vdata().iter().map(|v| v.data().len()).sum())
                    }
                }

                let mut state = ctx.acquire();

                let write_buffer = state.global.buffers.get_mut(&buffer_id).unwrap();
                let mut total_written = 0;
                for iovec in msg.vdata() {
                    if write_buffer.is_full() {
                        break;
                    }
                    total_written += write_buffer.write(iovec.data());
                }

                if write_buffer.is_full() {
                    state.lower_polled(&write_polled);
                }

                if let Some(read_polled) = read_polled {
                    state.raise_polled(&read_polled);
                }

                Ok(total_written)
            }
            _ => Err(SocketError::InvalidState),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SocketError {
    /// The socket's connection was in an invalid state relative to the operation being performed.
    InvalidState,
    /// The address supplied was not well-formed.
    InvalidAddress,
    /// A read, write or shutdown operation was attempted on a socket that was not connected.
    NotConnected,
    /// The address the socket attempted to bind to was already bound.
    AddressInUse,
    /// The address which a `connect()` or `sendto()` was addressed to did not have any server listening on it.
    AddressNotListening,
    /// The queue of incoming connections was filled before they could be handled.
    ConnectionQueueFull,
    /// A connection has been initiated and would block.
    ConnectInProgress,
    /// A connection has previously been initiated and would block.
    ConnectAlreadyStarted,
    /// A connection has previously been completed.
    ConnectAlreadyCompleted,
    /// No connection was immediately ready to be accepted by the server.
    AcceptPending,
    /// A requested non-blocking operation would cause the socket to block.
    WouldBlock,
}

impl From<SocketError> for DescriptorError {
    fn from(value: SocketError) -> Self {
        match value {
            SocketError::InvalidState => DescriptorError::InvalidInput,
            SocketError::InvalidAddress => DescriptorError::InvalidInput,
            SocketError::NotConnected => DescriptorError::NotConnected,
            SocketError::AddressInUse => DescriptorError::AddressInUse,
            SocketError::AddressNotListening => DescriptorError::ConnectionRefused,
            SocketError::ConnectionQueueFull => DescriptorError::ConnectionRefused,
            SocketError::ConnectInProgress => DescriptorError::ConnectInProgress,
            SocketError::ConnectAlreadyStarted => DescriptorError::WouldBlock,
            SocketError::ConnectAlreadyCompleted => DescriptorError::IsConnected,
            SocketError::AcceptPending => DescriptorError::WouldBlock,
            SocketError::WouldBlock => DescriptorError::WouldBlock,
        }
    }
}

impl FfiOutput for Result<usize, SocketError> {
    type OutputType = libc::c_int;

    fn out(&self) -> Self::OutputType {
        match self {
            Ok(val) => {
                Self::set_errno(0);
                return *val as libc::c_int;
            }
            Err(SocketError::InvalidState) => Self::set_errno(libc::EINVAL),
            Err(SocketError::InvalidAddress) => Self::set_errno(libc::EINVAL),
            Err(SocketError::NotConnected) => Self::set_errno(libc::ENOTCONN),
            Err(SocketError::AddressInUse) => Self::set_errno(libc::EADDRINUSE),
            Err(SocketError::AddressNotListening) => Self::set_errno(libc::ECONNREFUSED),
            Err(SocketError::ConnectionQueueFull) => Self::set_errno(libc::ECONNREFUSED),
            Err(SocketError::ConnectInProgress) => Self::set_errno(libc::EINPROGRESS),
            Err(SocketError::ConnectAlreadyStarted) => Self::set_errno(libc::EALREADY),
            Err(SocketError::ConnectAlreadyCompleted) => Self::set_errno(libc::EISCONN),
            Err(SocketError::AcceptPending) => Self::set_errno(libc::EAGAIN),
            Err(SocketError::WouldBlock) => Self::set_errno(libc::EAGAIN),
        }

        -1
    }

    fn display(&self) -> &'static str {
        match self {
            Ok(0) => "0",
            Ok(_) => ">0",
            Err(SocketError::InvalidState) => "-1 (EINVAL)",
            Err(SocketError::InvalidAddress) => "-1 (EINVAL)",
            Err(SocketError::NotConnected) => "-1 (ENOTCONN)",
            Err(SocketError::AddressInUse) => "-1 (EADDRINUSE)",
            Err(SocketError::AddressNotListening | SocketError::ConnectionQueueFull) => {
                "-1 (ECONNREFUSED)"
            }
            Err(SocketError::ConnectInProgress) => "-1 (EINPROGRESS)",
            Err(SocketError::ConnectAlreadyStarted) => "-1 (EALREADY)",
            Err(SocketError::ConnectAlreadyCompleted) => "-1 (EISCONN)",
            Err(SocketError::AcceptPending) => "-1 (EAGAIN)",
            Err(SocketError::WouldBlock) => "-1 (EAGAIN)",
        }
    }
}

impl FfiOutput for Result<SockAddr, SocketError> {
    type OutputType = libc::c_int;

    fn out(&self) -> Self::OutputType {
        self.clone().map(|_| 0).out()
    }

    fn display(&self) -> &'static str {
        self.clone().map(|_| 0).display()
    }
}

impl FfiOutput for Result<(RawFd, SockAddr), SocketError> {
    type OutputType = libc::c_int;

    fn out(&self) -> Self::OutputType {
        let r: Result<usize, SocketError> = match self {
            Ok((fd, _)) => return *fd,
            Err(e) => Err(*e),
        };

        r.out()
    }

    fn display(&self) -> &'static str {
        let r = match self {
            Ok(_) => Ok(()),
            Err(e) => Err(*e),
        };

        r.display()
    }
}

impl FfiOutput for Result<(), SocketError> {
    type OutputType = libc::c_int;

    fn out(&self) -> Self::OutputType {
        self.clone().map(|_| 0).out()
    }

    fn display(&self) -> &'static str {
        self.clone().map(|_| 0).display()
    }
}

// New code here:

pub struct SocketCreateEvent {
    pub domain: AddressFamily,
    pub socket_type: SocketType,
    pub protocol: TransportProtocol,
    pub nonblocking: bool,
    pub cloexec: bool,
}

impl Event for SocketCreateEvent {
    type Success = DescriptorId;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let fd = DescriptorId::from_raw_fd(crate::create_descriptor());

        let socket_id = state
            .global
            .sockets
            .allocate(SocketInfo {
                fd_count: 1,
                socktype: self.socket_type,
                protocol: self.protocol,
                local_addr: LocalAddress::Ephemeral(self.domain),
                state: match self.socket_type {
                    SocketType::SeqPacket | SocketType::Stream => {
                        SocketState::Unassociated(UnassociatedSocket { reuse_port: false })
                    }
                    SocketType::Datagram => {
                        let recv_buf = state.global.buffers.allocate(Buffer::new()).unwrap();
                        let read_polled = state
                            .global
                            .polled_events
                            .allocate(PolledInfo::new())
                            .unwrap();
                        let write_polled = state
                            .global
                            .polled_events
                            .allocate(PolledInfo::new())
                            .unwrap();

                        SocketState::Connectionless(ConnectionlessSocket {
                            reuse_port: false,
                            backend: ConnectionlessBackend::Peered(RegularConnectionless {
                                recv_buf,
                                read_polled,
                                write_polled,
                            }),
                            rem_addr: None,
                        })
                    }
                },
            })
            .unwrap();

        state
            .local
            .fds
            .allocate_with_key(
                fd,
                DescriptorInfo {
                    close_on_exec: self.cloexec,
                    nonblocking: self.nonblocking,
                    is_passthrough: false,
                    resource: FdResource::Socket(socket_id),
                },
            )
            .unwrap();

        Outcome::Success(fd)
    }
}

pub struct SocketCreatePairEvent {
    pub domain: AddressFamily,
    pub socket_type: SocketType,
    pub protocol: TransportProtocol,
    pub nonblocking: bool,
    pub cloexec: bool,
}

impl Event for SocketCreatePairEvent {
    type Success = (DescriptorId, DescriptorId);
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let addr1 = state.global.ephemeral_address(self.domain, self.protocol);
        let fd1 = DescriptorId::from_raw_fd(crate::create_descriptor());
        let recv_buf1 = state.global.buffers.allocate(Buffer::new()).unwrap();
        let read_polled1 = state
            .global
            .polled_events
            .allocate(PolledInfo::new())
            .unwrap();
        let write_polled1 = state
            .global
            .polled_events
            .allocate(PolledInfo::new())
            .unwrap();

        let addr2 = state.global.ephemeral_address(self.domain, self.protocol);
        let fd2 = DescriptorId::from_raw_fd(crate::create_descriptor());
        let recv_buf2 = state.global.buffers.allocate(Buffer::new()).unwrap();
        let read_polled2 = state
            .global
            .polled_events
            .allocate(PolledInfo::new())
            .unwrap();
        let write_polled2 = state
            .global
            .polled_events
            .allocate(PolledInfo::new())
            .unwrap();

        let socket_id1 = state
            .global
            .sockets
            .allocate(SocketInfo {
                fd_count: 1,
                socktype: self.socket_type,
                protocol: self.protocol,
                local_addr: LocalAddress::Assigned(addr1.addr().clone()),
                state: match self.socket_type {
                    SocketType::SeqPacket | SocketType::Stream => {
                        SocketState::Connected(ConnectedSocket {
                            backend: ConnectedBackend::Peered(RegularConnected {
                                peer: None,
                                recv_buf: recv_buf1,
                                read_polled: read_polled1,
                                write_polled: write_polled1,
                            }),
                            rem_addr: addr2.clone(),
                            peer_closed: false,
                        })
                    }
                    SocketType::Datagram => SocketState::Connectionless(ConnectionlessSocket {
                        reuse_port: false,
                        backend: ConnectionlessBackend::Peered(RegularConnectionless {
                            recv_buf: recv_buf1,
                            read_polled: read_polled1,
                            write_polled: write_polled1,
                        }),
                        rem_addr: Some(addr2.clone()),
                    }),
                },
            })
            .unwrap();

        let socket_id2 = state
            .global
            .sockets
            .allocate(SocketInfo {
                fd_count: 1,
                socktype: self.socket_type,
                protocol: self.protocol,
                local_addr: LocalAddress::Assigned(addr2.addr().clone()),
                state: match self.socket_type {
                    SocketType::SeqPacket | SocketType::Stream => {
                        SocketState::Connected(ConnectedSocket {
                            backend: ConnectedBackend::Peered(RegularConnected {
                                peer: Some(socket_id1.clone()),
                                recv_buf: recv_buf2,
                                read_polled: read_polled2,
                                write_polled: write_polled2,
                            }),
                            rem_addr: addr1.clone(),
                            peer_closed: false,
                        })
                    }
                    SocketType::Datagram => SocketState::Connectionless(ConnectionlessSocket {
                        reuse_port: false,
                        backend: ConnectionlessBackend::Peered(RegularConnectionless {
                            recv_buf: recv_buf2,
                            read_polled: read_polled2,
                            write_polled: write_polled2,
                        }),
                        rem_addr: Some(addr1.clone()),
                    }),
                },
            })
            .unwrap();

        match &mut state.global.sockets.get_mut(&socket_id2).unwrap().state {
            SocketState::Connected(connected_socket) => match &mut connected_socket.backend {
                crate::backend::IoBackend::Peered(p) => p.peer = Some(socket_id2.clone()),
                _ => unreachable!(),
            },
            _ => (),
        }

        state
            .local
            .fds
            .allocate_with_key(
                fd1,
                DescriptorInfo {
                    close_on_exec: self.cloexec,
                    nonblocking: self.nonblocking,
                    is_passthrough: false,
                    resource: FdResource::Socket(socket_id1.clone()),
                },
            )
            .unwrap();

        state
            .local
            .fds
            .allocate_with_key(
                fd2,
                DescriptorInfo {
                    close_on_exec: self.cloexec,
                    nonblocking: self.nonblocking,
                    is_passthrough: false,
                    resource: FdResource::Socket(socket_id2.clone()),
                },
            )
            .unwrap();

        let mut bound_sockets1 = Deque::new();
        bound_sockets1.push_back(socket_id1).unwrap();

        state.global.socket_locations.insert(
            addr1,
            TransportLocationInfo {
                reuse_port: false,
                bound_sockets: bound_sockets1,
                pending: None,
            },
        );

        let mut bound_sockets2 = Deque::new();
        bound_sockets2.push_back(socket_id2).unwrap();

        state.global.socket_locations.insert(
            addr2,
            TransportLocationInfo {
                reuse_port: false,
                bound_sockets: bound_sockets2,
                pending: None,
            },
        );

        Outcome::Success((fd1, fd2))
    }
}

pub struct SocketBindEvent {
    descriptor_id: DescriptorId,
    sockaddr: SockAddr,
}

impl SocketBindEvent {
    pub fn new(descriptor_id: DescriptorId, sockaddr: SockAddr) -> Self {
        Self {
            descriptor_id,
            sockaddr,
        }
    }

    fn next_ephemeral_port(&self, state: &mut FizzleState) -> u16 {
        let port = state.global.next_ephemeral_port;
        if state.global.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
            panic!("ephemeral ports exhausted")
        } else {
            state.global.next_ephemeral_port += 1;
        }

        port
    }
}

impl Event for SocketBindEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let mut sockaddr = self.sockaddr.clone();

        // If port is 0, bind to an ephemerally-chosen port
        match &mut sockaddr {
            SockAddr::Ipv4(v4_addr) if v4_addr.port() == 0 => {
                v4_addr.set_port(self.next_ephemeral_port(state));
            }
            SockAddr::Ipv6(v6_addr) if v6_addr.port() == 0 => {
                v6_addr.set_port(self.next_ephemeral_port(state));
            }
            _ => (),
        }

        let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();

        let transport_addr = TransportAddress {
            sockaddr: sockaddr.clone(),
            protocol: socket_info.protocol,
        };

        match &socket_info.local_addr {
            LocalAddress::Ephemeral(_) => (),
            _ => {
                log::error!("attempt to re-bind socket that already had `bind()` called on it");
                return Outcome::Error(Errno::EINVAL);
            }
        }

        match &socket_info.state {
            SocketState::Server(_) | SocketState::Connecting(_) | SocketState::Connected(_) => {
                log::error!("socket in invalid state when binding (`listen()` or `connect()` already called)");
                return Outcome::Error(Errno::EINVAL);
            }
            _ => (),
        }

        let reuse_port = match &socket_info.state {
            SocketState::Unassociated(u) => u.reuse_port,
            SocketState::Connectionless(c) => c.reuse_port,
            _ => unreachable!(),
        };

        let wildcard_bound = if let Some(wildcard) = transport_addr.wildcard() {
            state
                .global
                .socket_locations
                .get(&wildcard)
                .map_or(false, |a| {
                    (!a.reuse_port || !reuse_port) || !a.bound_sockets.is_empty()
                })
        } else {
            false
        };

        match state.global.socket_locations.entry(transport_addr) {
            Entry::Occupied(mut o) => {
                let location_info = o.get_mut();

                if (reuse_port && location_info.reuse_port)
                    || (!wildcard_bound && location_info.bound_sockets.is_empty())
                {
                    location_info
                        .bound_sockets
                        .push_back(socket_id.clone())
                        .unwrap();
                    location_info.reuse_port |= reuse_port;
                } else {
                    log::warn!("socket attempted to bind to bound address {}", o.key());
                    return Outcome::Error(Errno::EADDRINUSE);
                }
            }
            Entry::Vacant(v) => {
                let mut bound_sockets = Deque::new();
                bound_sockets.push_back(socket_id.clone()).unwrap();

                v.insert(TransportLocationInfo {
                    reuse_port,
                    bound_sockets,
                    pending: None,
                })
                .unwrap();
            }
        }

        // Swap the address out properly
        let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();
        mem::replace(
            &mut socket_info.local_addr,
            LocalAddress::Assigned(sockaddr),
        );

        Outcome::Success(())
    }
}

pub struct SocketListenEvent {
    descriptor_id: DescriptorId,
    backlog: libc::c_int, // Not actually used
}

impl SocketListenEvent {
    pub fn new(descriptor_id: DescriptorId, backlog: libc::c_int) -> Self {
        Self {
            descriptor_id,
            backlog,
        }
    }
}

impl Event for SocketListenEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();

        let SocketState::Unassociated(_) = &mut socket_info.state else {
            log::error!(
                "calling listen() on a connected or listening socket unsupported by Fizzle"
            );
            return Outcome::Error(Errno::EINVAL);
        };

        let addr = get_or_assign_local(socket_id.clone(), state);

        // Allocate server context and set up polling
        let ready_to_connect = state
            .global
            .polled_events
            .allocate(PolledInfo::new())
            .unwrap();

        if state
            .global
            .socket_locations
            .get_mut(&addr)
            .unwrap()
            .pending
            .is_some()
        {
            state.raise_polled(&ready_to_connect);
        }

        let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();

        // In the case of an ephemeral address, the concrete address now needs to be assigned to the socket
        socket_info.local_addr = LocalAddress::Assigned(addr.addr().clone());

        socket_info.state = SocketState::Server(ServerSocket {
            backend: ServerBackend::Peered(()),
            connecting: heapless::Deque::new(),
            ready_to_connect,
        });

        Outcome::Success(())
    }
}

pub enum SocketConnectState {
    Start,
    Finish(Rc<PollerId>),
}

pub struct SocketConnectEvent {
    descriptor_id: DescriptorId,
    dst_addr: SockAddr,
    state: SocketConnectState,
}

impl SocketConnectEvent {
    pub fn new(descriptor_id: DescriptorId, dst_addr: SockAddr) -> Self {
        Self {
            descriptor_id,
            dst_addr,
            state: SocketConnectState::Start,
        }
    }
}

impl Event for SocketConnectEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &self.state {
            SocketConnectState::Start => {
                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    return Outcome::Error(Errno::EBADF);
                };

                let nonblocking = fd_info.nonblocking;

                let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::ENOTSOCK);
                };

                let socket_info = state.global.sockets.get(&socket_id).unwrap();
                match &socket_info.state {
                    SocketState::Unassociated(_) => {
                        let protocol = socket_info.protocol;

                        get_or_assign_local(socket_id.clone(), state);

                        let dst_addr = TransportAddress {
                            sockaddr: self.dst_addr.clone(),
                            protocol,
                        };

                        let server_socket_id: Rc<SocketId>;

                        // TODO: distribute evenly across multiple listening sockets
                        if let Some(socket_id) = state
                            .global
                            .socket_locations
                            .get(&dst_addr)
                            .and_then(|l| l.bound_sockets.front())
                        {
                            // The exact address is bound
                            server_socket_id = socket_id.clone();
                        } else if let Some(socket_id) = dst_addr
                            .wildcard()
                            .and_then(|t| state.global.socket_locations.get(&t))
                            .and_then(|l| l.bound_sockets.front())
                        {
                            // The wildcard address is bound
                            server_socket_id = socket_id.clone();
                        } else {
                            // No socket is bound to the given address...
                            log::warn!(
                                "connect() on address {} failed--no server listening",
                                &dst_addr
                            );
                            return Outcome::Error(Errno::ECONNREFUSED);
                        }

                        let socket_info = state.global.sockets.get_mut(&server_socket_id).unwrap();
                        let SocketState::Server(server_info) = &mut socket_info.state else {
                            log::error!("inconsistent Fizzle state--socket bound to connect() address {} not in listening state", &dst_addr);
                            return Outcome::Error(Errno::ECONNREFUSED);
                        };

                        let server_backend = server_info.backend.clone();
                        let connecting = &mut server_info.connecting;

                        let connected_backend = match server_backend {
                            ServerBackend::Passthrough => unreachable!(),
                            ServerBackend::Peered(()) => {
                                let Ok(_) = connecting.push_back(socket_id.clone()) else {
                                    return Outcome::Error(Errno::ECONNABORTED);
                                };

                                let server_poll = server_info.ready_to_connect.clone();
                                state.raise_polled(&server_poll);

                                let client_poll = state
                                    .global
                                    .polled_events
                                    .allocate(PolledInfo::new())
                                    .unwrap();

                                state.global.sockets.get_mut(&socket_id).unwrap().state =
                                    SocketState::Connecting(ConnectingSocket {
                                        backend: ConnectingBackend::Peered(()),
                                        connect_polled: client_poll.clone(),
                                    });

                                // The server side sets this backend to conected, so we don't need to here.
                                if nonblocking {
                                    return Outcome::Error(Errno::EINPROGRESS);
                                } else {
                                    if state.polled_is_ready(&client_poll) {
                                        panic!("`connect()` poller in unexpected state");
                                    } else {
                                        let poller_id = state.new_poller();
                                        state.register_poller(poller_id.clone(), client_poll);
                                        self.state = SocketConnectState::Finish(poller_id);

                                        // TODO: for `SO_SNDTIMEO`, this should be Some()
                                        return Outcome::Yield(None);
                                    }
                                }
                            }
                            ServerBackend::Plugin(plugin_id) => {
                                // Create new plugin
                                let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                                let endpoint = plugin_info.endpoint.clone();
                                let module_id = plugin_info.module_id.clone();
                                let connect_plugin_id =
                                    state.global.add_plugin(endpoint, module_id);
                                ConnectedBackend::Plugin(connect_plugin_id)
                            }
                            ServerBackend::Sink => ConnectedBackend::Sink,
                            ServerBackend::Fuzz(_) => {
                                ConnectedBackend::Fuzz(state.global.add_fuzz_endpoint())
                            }
                            ServerBackend::NullSink => ConnectedBackend::NullSink,
                            ServerBackend::Feedback(()) => {
                                ConnectedBackend::Feedback(StandardFeedback {
                                    buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                                    read_polled: state
                                        .global
                                        .polled_events
                                        .allocate(PolledInfo::new())
                                        .unwrap(),
                                    write_polled: state
                                        .global
                                        .polled_events
                                        .allocate(PolledInfo::new_raised())
                                        .unwrap(),
                                })
                            }
                        };

                        state.global.sockets.get_mut(&socket_id).unwrap().state =
                            SocketState::Connected(ConnectedSocket {
                                backend: connected_backend,
                                rem_addr: dst_addr,
                                peer_closed: false,
                            });

                        Outcome::Success(())
                    }
                    SocketState::Server(_) => {
                        log::error!("connect() called on listening socket--unsupported by Fizzle");
                        Outcome::Error(Errno::EINVAL)
                    }
                    SocketState::PendingConnection(_) => unreachable!(),
                    SocketState::Connecting(c) => {
                        if nonblocking {
                            Outcome::Error(Errno::EINPROGRESS)
                        } else {
                            let client_poll = c.connect_polled.clone();

                            if state.polled_is_ready(&client_poll) {
                                return Outcome::Continue;
                            } else {
                                let poller_id = state.new_poller();
                                state.register_poller(poller_id.clone(), client_poll);
                                self.state = SocketConnectState::Finish(poller_id);

                                // TODO: for `SO_SNDTIMEO`, this should be Some()
                                return Outcome::Yield(None);
                            }
                        }
                    }
                    SocketState::Connected(_) => Outcome::Error(Errno::EISCONN),
                    SocketState::Connectionless(_) => unreachable!(),
                }
            }
            SocketConnectState::Finish(poller_id) => {
                state.delete_poller(poller_id.clone());
                Outcome::Success(())
            }
        }
    }
}

pub enum SocketAcceptState {
    Start,
    Blocked(Rc<PollerId>),
    Finish(Rc<SocketId>, TransportAddress),
}

pub struct SocketAcceptEvent {
    descriptor_id: DescriptorId,
    nonblock: bool,
    cloexec: bool,
    state: SocketAcceptState,
}

impl SocketAcceptEvent {
    pub fn new(descriptor_id: DescriptorId, nonblock: bool, cloexec: bool) -> Self {
        Self {
            descriptor_id,
            nonblock,
            cloexec,
            state: SocketAcceptState::Start,
        }
    }
}

impl Event for SocketAcceptEvent {
    type Success = (DescriptorId, SockAddr);
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &self.state {
            SocketAcceptState::Start => {
                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    return Outcome::Error(Errno::EBADF);
                };

                let nonblocking = fd_info.nonblocking;

                let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::ENOTSOCK);
                };

                let server_socket_info = state.global.sockets.get_mut(&socket_id).unwrap();
                let SocketState::Server(server_info) = &mut server_socket_info.state else {
                    log::error!("accept() called on non-listening socket");
                    return Outcome::Error(Errno::EINVAL);
                };

                let server_poll = server_info.ready_to_connect.clone();

                let has_connecting = !server_info.connecting.is_empty();
                let protocol = server_socket_info.protocol;
                let LocalAddress::Assigned(sockaddr) = server_socket_info.local_addr.clone() else {
                    unreachable!()
                };

                let server_address = TransportAddress { sockaddr, protocol };

                let ready_to_connect = server_info.ready_to_connect.clone();

                let bound_info = state.global.socket_locations.get(&server_address).unwrap();
                if let Some(PendingInfo { client, poll }) = bound_info.pending.clone() {
                    get_or_assign_local(client.clone(), state);

                    let client_socket_info = state.global.sockets.get_mut(&client).unwrap();
                    let SocketState::PendingConnection(pending_info) =
                        &mut client_socket_info.state
                    else {
                        unreachable!()
                    };

                    // Update the linked list of pending clients
                    match pending_info.next_pending.clone() {
                        Some(pending_id) => {
                            state
                                .global
                                .socket_locations
                                .get_mut(&server_address)
                                .unwrap()
                                .pending
                                .as_mut()
                                .unwrap()
                                .client = pending_id
                        }
                        None => {
                            state
                                .global
                                .socket_locations
                                .get_mut(&server_address)
                                .unwrap()
                                .pending = None;

                            if !has_connecting {
                                state.lower_polled(&ready_to_connect);
                            }
                        }
                    }

                    state.raise_polled(&poll);

                    self.state = SocketAcceptState::Finish(client, server_address);
                    Outcome::Continue
                } else {
                    let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();
                    let SocketState::Server(server_info) = &mut socket_info.state else {
                        unreachable!()
                    };

                    if let Some(connecting_id) = server_info.connecting.pop_front() {
                        if server_info.connecting.is_empty() {
                            // TODO: this used to be `len() == 1`--why?
                            state.lower_polled(&ready_to_connect);
                        }

                        let connecting_socket_info =
                            state.global.sockets.get_mut(&socket_id).unwrap();
                        let SocketState::Connecting(connecting_info) =
                            &mut connecting_socket_info.state
                        else {
                            unreachable!()
                        };

                        let connect_polled = connecting_info.connect_polled.clone();
                        state.raise_polled(&connect_polled);

                        get_or_assign_local(connecting_id.clone(), state);
                        self.state =
                            SocketAcceptState::Finish(connecting_id.clone(), server_address);
                        Outcome::Continue
                    } else if nonblocking {
                        Outcome::Error(Errno::EAGAIN)
                    } else {
                        if state.polled_is_ready(&server_poll) {
                            panic!("`accept()` poller in unexpected state");
                        } else {
                            let poller_id = state.new_poller();
                            state.register_poller(poller_id.clone(), server_poll);

                            self.state = SocketAcceptState::Blocked(poller_id);
                            // TODO: for `SO_SNDTIMEO`, this should be Some()
                            Outcome::Yield(None)
                        }
                    }
                }
            }
            SocketAcceptState::Blocked(poller_id) => {
                state.delete_poller(poller_id.clone());

                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    log::error!("socket unexpectedly closed during `accept()`");
                    return Outcome::Error(Errno::EBADF);
                };

                let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::ENOTSOCK);
                };

                let server_address = get_or_assign_local(socket_id.clone(), state);

                let server_socket_info = state.global.sockets.get_mut(&socket_id).unwrap();
                let SocketState::Server(server_info) = &mut server_socket_info.state else {
                    log::error!("socket state unexpectedly changed during `accept()`");
                    return Outcome::Error(Errno::EINVAL);
                };

                let polled_id = server_info.ready_to_connect.clone();
                let more_connecting = !server_info.connecting.is_empty();

                let connecting_id = server_info.connecting.pop_front().unwrap();
                let connecting_socket_info = state.global.sockets.get_mut(&connecting_id).unwrap();
                let SocketState::Connecting(_) = &connecting_socket_info.state else {
                    unreachable!()
                };

                if !more_connecting {
                    state.lower_polled(&polled_id);
                }

                get_or_assign_local(connecting_id.clone(), state);

                self.state = SocketAcceptState::Finish(connecting_id.clone(), server_address);
                Outcome::Continue
            }
            SocketAcceptState::Finish(connecting_id, server_address) => {
                // `connecting` corresponds to the client, while `accepting` corresponds to the new
                // socket created by the server to accept the connection.

                let connecting_address = get_or_assign_local(connecting_id.clone(), state);

                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    log::error!("socket unexpectedly closed during `accept()`");
                    return Outcome::Error(Errno::EBADF);
                };

                let close_on_exec = fd_info.close_on_exec;
                let nonblocking = fd_info.nonblocking;

                let connecting_socket_info = state.global.sockets.get_mut(&connecting_id).unwrap();
                let SocketState::Connecting(connecting_info) = &mut connecting_socket_info.state
                else {
                    unreachable!()
                };

                let socktype = connecting_socket_info.socktype;
                let protocol = connecting_socket_info.protocol;

                let connecting_backend = connecting_info.backend.clone();
                let connecting_polled = connecting_info.connect_polled.clone();

                let accepting_backend = match connecting_backend {
                    ConnectingBackend::Passthrough => unreachable!(),
                    ConnectingBackend::Peered(()) => ConnectedBackend::Peered(RegularConnected {
                        peer: Some(connecting_id.clone()),
                        recv_buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                        read_polled: state
                            .global
                            .polled_events
                            .allocate(PolledInfo::new())
                            .unwrap(),
                        write_polled: state
                            .global
                            .polled_events
                            .allocate(PolledInfo::new_raised())
                            .unwrap(),
                    }),
                    ConnectingBackend::Feedback(()) => {
                        ConnectedBackend::Feedback(StandardFeedback {
                            buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                            read_polled: state
                                .global
                                .polled_events
                                .allocate(PolledInfo::new())
                                .unwrap(),
                            write_polled: state
                                .global
                                .polled_events
                                .allocate(PolledInfo::new_raised())
                                .unwrap(),
                        })
                    }
                    ConnectingBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                        let endpoint = plugin_info.endpoint.clone();
                        let module_id = plugin_info.module_id.clone();
                        let connect_plugin_id = state.global.add_plugin(endpoint, module_id);
                        ConnectedBackend::Plugin(connect_plugin_id)
                    }
                    ConnectingBackend::Sink => ConnectedBackend::Sink,
                    ConnectingBackend::NullSink => ConnectedBackend::NullSink,
                    ConnectingBackend::Fuzz(endpoint) => ConnectedBackend::Fuzz(endpoint),
                };

                let accepting_id = if let ConnectedBackend::Peered(_) = accepting_backend {
                    let accepting_id = state
                        .global
                        .sockets
                        .allocate(SocketInfo {
                            fd_count: 1,
                            socktype,
                            protocol,
                            local_addr: LocalAddress::Assigned(server_address.sockaddr.clone()),
                            state: SocketState::Connected(ConnectedSocket {
                                rem_addr: connecting_address.clone(),
                                backend: accepting_backend,
                                peer_closed: false,
                            }),
                        })
                        .unwrap();

                    let connected_backend = ConnectedBackend::Peered(RegularConnected {
                        peer: Some(accepting_id.clone()),
                        recv_buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                        read_polled: state
                            .global
                            .polled_events
                            .allocate(PolledInfo::new())
                            .unwrap(),
                        write_polled: state
                            .global
                            .polled_events
                            .allocate(PolledInfo::new_raised())
                            .unwrap(),
                    });

                    state.global.sockets.get_mut(&connecting_id).unwrap().state =
                        SocketState::Connected(ConnectedSocket {
                            rem_addr: server_address.clone(),
                            backend: connected_backend,
                            peer_closed: false,
                        });

                    accepting_id
                } else {
                    // The connecting socket was emulated in some way (`fuzz`, `sink` or the like).
                    // Convert the connecting socket into the accepted socket--we don't need two peered sockets.

                    let connecting_socket_info =
                        state.global.sockets.get_mut(&connecting_id).unwrap();

                    connecting_socket_info.local_addr =
                        LocalAddress::Assigned(server_address.sockaddr.clone());
                    connecting_socket_info.state = SocketState::Connected(ConnectedSocket {
                        rem_addr: connecting_address.clone(),
                        backend: accepting_backend,
                        peer_closed: false,
                    });

                    connecting_id.clone()
                };

                // Let the connecting socket know it's been connected
                state.raise_polled(&connecting_polled);

                let new_fd = DescriptorId::from_raw_fd(crate::create_descriptor());
                // The two sockets are now joined--add a file descriptor to the accepted socket
                state
                    .local
                    .fds
                    .allocate_with_key(
                        new_fd,
                        DescriptorInfo {
                            close_on_exec,
                            is_passthrough: false,
                            nonblocking,
                            resource: FdResource::Socket(accepting_id),
                        },
                    )
                    .unwrap();

                Outcome::Success((new_fd, connecting_address.sockaddr.clone()))
            }
        }
    }
}

pub struct SocketGetNameEvent {
    descriptor_id: DescriptorId,
}

impl SocketGetNameEvent {
    pub fn new(descriptor_id: DescriptorId) -> Self {
        Self { descriptor_id }
    }
}

impl Event for SocketGetNameEvent {
    type Success = Result<SockAddr, AddressFamily>;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let sockaddr = match &state.global.sockets.get(&socket_id).unwrap().local_addr {
            LocalAddress::Ephemeral(address_family) => Err(*address_family),
            LocalAddress::Assigned(sock_addr) => Ok(sock_addr.clone()),
        };

        Outcome::Success(sockaddr)
    }
}

#[repr(C)]
struct SctpRtoInfo {
    pub srto_assoc_id: libc::sctp_assoc_t,
    pub srto_initial: u32,
    pub srto_max: u32,
    pub srto_min: u32,
}

#[repr(C)]
pub struct SctpGetaddrs {
    pub assoc_id: libc::sctp_assoc_t, // input
    pub addr_num: i32,                // output
    pub addrs: *mut u8,               // output, variable size
}

#[repr(packed)]
pub struct SctpPeerAddrParams {
    pub spp_assoc_id: libc::sctp_assoc_t,
    pub spp_address: libc::sockaddr_storage,
    pub spp_hbinterval: u32,
    pub spp_pathmaxrxt: u16,
    pub spp_pathmtu: u32,
    pub spp_sackdelay: u32,
    pub spp_flags: u32,
    pub spp_ipv6_flowlabel: u32,
    pub spp_dscp: u8,
}

#[repr(C)]
pub struct SctpAssocParams {
    pub sasoc_assoc_id: libc::sctp_assoc_t,
    pub sasoc_asocmaxrxt: u16,
    pub sasoc_peer_rwnd: u32,
    pub sasoc_local_rwnd: u32,
    pub sasoc_cookie_life: u32,
}

#[repr(C)]
pub struct SctpInitMsg {
    pub sinit_num_ostreams: u16,
    pub sinit_max_instreams: u16,
    pub sinit_max_attempts: u16,
    pub sinit_max_init_timeo: u16,
}

#[repr(C)]
pub struct SctpEventSubscribe {
    pub sctp_data_io_event: u8,
    pub sctp_association_event: u8,
    pub sctp_address_event: u8,
    pub sctp_send_failure_event: u8,
    pub sctp_peer_error_event: u8,
    pub sctp_shutdown_event: u8,
    pub sctp_partial_delivery_event: u8,
    pub sctp_adaptation_layer_event: u8,
    pub sctp_authentication_event: u8,
    pub sctp_sender_dry_event: u8,
    pub sctp_stream_reset_event: u8,
    pub sctp_assoc_reset_event: u8,
    pub sctp_stream_change_event: u8,
    pub sctp_send_failure_event_event: u8,
}

pub const SOL_SCTP: i32 = 132;
pub const SCTP_SOCKOPT_BINDX_ADD: i32 = 100;
pub const SCTP_SOCKOPT_BINDX_REM: i32 = 101;
// const SCTP_SOCKOPT_PEELOFF: i32 = 102;

pub const SCTP_SOCKOPT_CONNECTX_OLD: i32 = 107;
pub const SCTP_GET_PEER_ADDRS: i32 = 108;
pub const SCTP_GET_LOCAL_ADDRS: i32 = 109;
pub const SCTP_SOCKOPT_CONNECTX: i32 = 110;
pub const SCTP_SOCKOPT_CONNECTX3: i32 = 111;
pub const SCTP_GET_ASSOC_STATS: i32 = 112;
pub const SCTP_PR_SUPPORTED: i32 = 113;

#[derive(Clone, Copy, Debug)]
pub enum OptLevel {
    Socket,
    Ip,
    Ipv6,
    Sctp,
    Tcp,
}

pub enum SocketOption {
    SocketIsListening(bool),
    SocketDontRoute(bool),
    SocketDomain(AddressFamily),
    SocketError(libc::c_int),
    SocketKeepalive(bool),
    SocketLinger(Option<u32>),
    SocketOobInline(bool),
    SocketZeroCopy(bool),
    SocketPriority(u32),
    SocketProtocol(TransportProtocol),
    SocketRecvBuffer(u32),
    SocketSendLowWatermark(u32),
    SocketRecvLowWatermark(u32),
    SocketReuseAddr(bool),
    SocketReusePort(bool),
    IpOptions(Vec<u8>),
    Ipv6Only(bool),
    SctpRtoInfo(SctpRtoInfo),
    SctpGetLocalAddrs(Vec<u8>),
    SctpInitMsg(SctpInitMsg),
    SctpNoDelay(bool),
    SctpAutoClose(bool),
    SctpDisableFragments(bool),
    SctpPeerAddrParams(SctpPeerAddrParams),
    SctpEvents(SctpEventSubscribe),
    SctpWantMappedV4Addr(bool),
    SctpFragmentInterleave(bool),
    SctpMaxSegment(u32),
    SctpAssocInfo(SctpAssocParams),
    TcpUserTimeout(Duration),
    TcpNoDelay(bool),
    TcpMss(u32),
}

impl SocketOption {
    pub fn encode(&self, out: &mut [MaybeUninit<u8>]) -> usize {
        match self {
            Self::IpOptions(v) | Self::SctpGetLocalAddrs(v) => {
                for (dst, src) in out.iter_mut().zip(v) {
                    dst.write(*src);
                }

                v.len()
            }
            Self::TcpUserTimeout(d) => {
                let millis: libc::c_int = d.as_millis().try_into().unwrap();
                let millis_bytes = millis.to_be_bytes();

                for (dst, src) in out.iter_mut().zip(millis_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&millis)
            }
            Self::TcpNoDelay(b)
            | Self::SocketIsListening(b)
            | Self::SocketDontRoute(b)
            | Self::SocketKeepalive(b)
            | Self::SocketOobInline(b)
            | Self::SocketZeroCopy(b)
            | Self::SocketReuseAddr(b)
            | Self::SocketReusePort(b)
            | Self::SctpNoDelay(b)
            | Self::SctpAutoClose(b)
            | Self::SctpDisableFragments(b)
            | Self::SctpWantMappedV4Addr(b)
            | Self::SctpFragmentInterleave(b)
            | Self::Ipv6Only(b) => {
                let flag: libc::c_int = match b {
                    true => 1,
                    false => 0,
                };

                let flag_bytes = flag.to_be_bytes();

                for (dst, src) in out.iter_mut().zip(flag_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&flag)
            }
            Self::TcpMss(u)
            | Self::SocketPriority(u)
            | Self::SocketRecvBuffer(u)
            | Self::SocketSendLowWatermark(u)
            | Self::SocketRecvLowWatermark(u)
            | Self::SctpMaxSegment(u) => {
                let i: libc::c_int = (*u).try_into().unwrap();
                let i_bytes = i.to_be_bytes();

                for (dst, src) in out.iter_mut().zip(i_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&i)
            }
            Self::SocketDomain(f) => {
                let domain = match f {
                    AddressFamily::Ipv4 => libc::AF_INET,
                    AddressFamily::Ipv6 => libc::AF_INET6,
                    AddressFamily::Unix => libc::AF_UNIX,
                };

                let domain_bytes = domain.to_be_bytes();

                for (dst, src) in out.iter_mut().zip(domain_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&domain)
            }
            Self::SocketError(error) => {
                let error_bytes = error.to_be_bytes();

                for (dst, src) in out.iter_mut().zip(error_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&error)
            }
            Self::SocketLinger(l) => {
                let linger = match *l {
                    None => libc::linger {
                        l_linger: 0,
                        l_onoff: 0,
                    },
                    Some(t) => libc::linger {
                        l_linger: t.try_into().unwrap(),
                        l_onoff: 1,
                    },
                };

                // SAFETY: u8 never should have alignment issues, so this should turn &linger to &[u8]
                let linger_bytes: &[u8] = unsafe { slice::from_ref(&linger).align_to().1 };
                assert!(
                    linger_bytes.len() == mem::size_of_val(&linger),
                    "align_to() failed to convert `libc::linger` to bytes"
                );

                for (dst, src) in out.iter_mut().zip(linger_bytes) {
                    dst.write(*src);
                }

                linger_bytes.len()
            }
            Self::SocketProtocol(p) => {
                let p_int: libc::c_int = match p {
                    TransportProtocol::Tcp => libc::IPPROTO_TCP, // TODO: what if this is zero?
                    TransportProtocol::Udp => libc::IPPROTO_UDP,
                    TransportProtocol::Sctp => libc::IPPROTO_SCTP,
                    TransportProtocol::Unix => 0,
                };

                let p_bytes = p_int.to_be_bytes();

                for (dst, src) in out.iter_mut().zip(p_bytes) {
                    dst.write(src);
                }

                p_bytes.len()
            }
            Self::SctpRtoInfo(rto_info) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &rto_info to &[u8]
                let rto_info_bytes: &[u8] = unsafe { slice::from_ref(&rto_info).align_to().1 };
                assert!(
                    rto_info_bytes.len() == mem::size_of_val(&rto_info),
                    "align_to() failed to convert `SctpRtoInfo` to bytes"
                );

                for (dst, src) in out.iter_mut().zip(rto_info_bytes) {
                    dst.write(*src);
                }

                rto_info_bytes.len()
            }
            Self::SctpInitMsg(init_msg) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &SctpInitMsg to &[u8]
                let init_msg_bytes: &[u8] = unsafe { slice::from_ref(&init_msg).align_to().1 };
                assert!(
                    init_msg_bytes.len() == mem::size_of_val(&init_msg),
                    "align_to() failed to convert `SctpInitMsg` to bytes"
                );

                for (dst, src) in out.iter_mut().zip(init_msg_bytes) {
                    dst.write(*src);
                }

                init_msg_bytes.len()
            }
            Self::SctpPeerAddrParams(addr_params) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &SctpPeerAddrParams to &[u8]
                let addr_param_bytes: &[u8] = unsafe { slice::from_ref(&addr_params).align_to().1 };
                assert!(
                    addr_param_bytes.len() == mem::size_of_val(&addr_params),
                    "align_to() failed to convert `SctpPeerAddrParams` to bytes"
                );

                for (dst, src) in out.iter_mut().zip(addr_param_bytes) {
                    dst.write(*src);
                }

                addr_param_bytes.len()
            }
            Self::SctpEvents(events_subscribe) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &SctpPeerAddrParams to &[u8]
                let events_subscribe_bytes: &[u8] =
                    unsafe { slice::from_ref(&events_subscribe).align_to().1 };
                assert!(
                    events_subscribe_bytes.len() == mem::size_of_val(&events_subscribe_bytes),
                    "align_to() failed to convert `SctpEventSubscribe` to bytes"
                );

                for (dst, src) in out.iter_mut().zip(events_subscribe_bytes) {
                    dst.write(*src);
                }

                events_subscribe_bytes.len()
            }
            Self::SctpAssocInfo(assoc_params) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &SctpPeerAddrParams to &[u8]
                let assoc_params_bytes: &[u8] =
                    unsafe { slice::from_ref(&assoc_params).align_to().1 };
                assert!(
                    assoc_params_bytes.len() == mem::size_of_val(&assoc_params_bytes),
                    "align_to() failed to convert `SctpAssocParams` to bytes"
                );

                for (dst, src) in out.iter_mut().zip(assoc_params_bytes) {
                    dst.write(*src);
                }

                assoc_params_bytes.len()
            }
        }
    }
}

pub enum OptInput {
    None,
    SctpAssocId(libc::sctp_assoc_t),
    SctpPeerAddrParams(libc::sctp_assoc_t, libc::sockaddr_storage),
}

pub struct SocketGetOptionEvent {
    descriptor_id: DescriptorId,
    optlevel: OptLevel,
    optname: libc::c_int,
    input: OptInput,
}

impl SocketGetOptionEvent {
    pub fn new(
        descriptor_id: DescriptorId,
        optlevel: OptLevel,
        optname: libc::c_int,
        input: OptInput,
    ) -> Self {
        Self {
            descriptor_id,
            optlevel,
            optname,
            input,
        }
    }
}

impl Event for SocketGetOptionEvent {
    type Success = SocketOption;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let socket_info = state.global.sockets.get(&socket_id).unwrap();
        let family = socket_info.local_addr.family();
        let protocol = socket_info.protocol;

        match (self.optlevel, family, protocol) {
            (OptLevel::Socket, _, _) => (),
            (OptLevel::Ip, AddressFamily::Ipv4, _) => (),
            (OptLevel::Ip, _, _) => return Outcome::Error(Errno::ENOPROTOOPT),
            (OptLevel::Ipv6, AddressFamily::Ipv6, _) => (),
            (OptLevel::Ipv6, _, _) => return Outcome::Error(Errno::ENOPROTOOPT),
            (OptLevel::Sctp, _, TransportProtocol::Sctp) => (),
            (OptLevel::Sctp, _, _) => return Outcome::Error(Errno::ENOPROTOOPT),
            (OptLevel::Tcp, _, TransportProtocol::Tcp) => (),
            (OptLevel::Tcp, _, _) => return Outcome::Error(Errno::ENOPROTOOPT),
        }

        match (self.optlevel, self.optname) {
            (OptLevel::Socket, libc::SO_ACCEPTCONN) => Outcome::Success(
                if let SocketState::Server(_) = &state.global.sockets.get(&socket_id).unwrap().state
                {
                    SocketOption::SocketIsListening(true)
                } else {
                    SocketOption::SocketIsListening(false)
                },
            ),
            (
                OptLevel::Socket,
                libc::SO_ATTACH_FILTER
                | libc::SO_LOCK_FILTER
                | libc::SO_DETACH_FILTER
                | libc::SO_ATTACH_BPF
                | libc::SO_ATTACH_REUSEPORT_CBPF
                | libc::SO_ATTACH_REUSEPORT_EBPF,
            ) => {
                // TODO: implement BPF filtering emulation
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Socket, libc::SO_BINDTODEVICE | libc::SO_BROADCAST | libc::SO_DEBUG) => {
                // TODO: implement BPF filtering emulation
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Socket, libc::SO_DONTROUTE) => {
                // TODO: implement assignment of this flag
                Outcome::Success(SocketOption::SocketDontRoute(true))
            }
            (OptLevel::Socket, libc::SO_DOMAIN) => Outcome::Success(SocketOption::SocketDomain(
                state
                    .global
                    .sockets
                    .get(&socket_id)
                    .unwrap()
                    .local_addr
                    .family(),
            )),
            (OptLevel::Socket, libc::SO_ERROR) => {
                // TODO: pass errors raised during polling here
                Outcome::Success(SocketOption::SocketError(0))
            }
            (OptLevel::Socket, libc::SO_KEEPALIVE) => {
                // TODO: implement assignment of this flag
                Outcome::Success(SocketOption::SocketKeepalive(false))
            }
            (OptLevel::Socket, libc::SO_LINGER) => {
                // TODO: implement assignment of this flag
                Outcome::Success(SocketOption::SocketLinger(Some(15)))
            }
            (OptLevel::Socket, libc::SO_OOBINLINE) => {
                // TODO: implement
                Outcome::Success(SocketOption::SocketOobInline(true))
            }
            (OptLevel::Socket, libc::SO_ZEROCOPY) => {
                // TODO: implement
                Outcome::Success(SocketOption::SocketZeroCopy(true))
            }
            (OptLevel::Socket, libc::SO_PRIORITY) => {
                // TODO: implement
                Outcome::Success(SocketOption::SocketPriority(6))
            }
            (OptLevel::Socket, libc::SO_PROTOCOL) => {
                Outcome::Success(SocketOption::SocketProtocol(
                    state.global.sockets.get(&socket_id).unwrap().protocol,
                ))
            }
            (OptLevel::Socket, libc::SO_RCVBUF) => {
                // TODO: implement
                Outcome::Success(SocketOption::SocketRecvBuffer(FIZZLE_BUFFER_LENGTH as u32))
            }
            (OptLevel::Socket, libc::SO_SNDLOWAT) => {
                // TODO: implement low watermarks for send, recv, poll
                log::error!("unimplemented socket option SO_SNDLOWAT for SOL_SOCKET");
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Socket, libc::SO_RCVLOWAT) => {
                // TODO: implement low/high watermarks for send, recv, poll
                log::error!("unimplemented socket option SO_RCVLOWAT for SOL_SOCKET");
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Socket, libc::SO_SNDTIMEO) => {
                // TODO: implement send timeout for send, recv
                log::error!("unimplemented socket option SO_SNDTIMEO for SOL_SOCKET");
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Socket, libc::SO_RCVTIMEO) => {
                // TODO: implement send timeout for send, recv, connect, accept
                log::error!("unimplemented socket option SO_RCVTIMEO for SOL_SOCKET");
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Socket, libc::SO_REUSEADDR) => {
                // TODO: implement
                Outcome::Success(SocketOption::SocketReuseAddr(true))
            }
            (OptLevel::Socket, libc::SO_REUSEPORT) => {
                let socket_info = state.global.sockets.get(&socket_id).unwrap();

                Outcome::Success(SocketOption::SocketReusePort(match &socket_info.state {
                    SocketState::Connectionless(connectionless_socket) => {
                        connectionless_socket.reuse_port
                    }
                    SocketState::Unassociated(unassociated_socket) => {
                        unassociated_socket.reuse_port
                    }
                    _ => {
                        let transport_addr = get_or_assign_local(socket_id, state);
                        state
                            .global
                            .socket_locations
                            .get(&transport_addr)
                            .unwrap()
                            .reuse_port
                    }
                }))
            }
            // TODO: implement SO_RXQ_OVFL, SO_TIMESTAMP, when implementing `cmsg`s
            (OptLevel::Socket, _) => {
                log::error!("unrecognized socket option {} for SOL_SOCKET", self.optname);
                panic!("unrecognized SOL_SOCKET sockopt {}", self.optname)
            }
            (OptLevel::Ip, libc::IP_OPTIONS) => {
                // TODO: implement assigning IP options to sockets
                Outcome::Success(SocketOption::IpOptions(Vec::new()))
            }
            (OptLevel::Ip, _) => {
                log::error!("unrecognized socket option for SOL_IP: {}", self.optname);
                panic!("unrecognized SOL_IP sockopt {}", self.optname)
            }
            (OptLevel::Ipv6, libc::IPV6_V6ONLY) => {
                // TODO: implement
                Outcome::Success(SocketOption::Ipv6Only(true))
            }
            (OptLevel::Ipv6, _) => {
                log::error!("unrecognized socket option for SOL_IP6: {}", self.optname);
                panic!("unrecognized SOL_IP6 sockopt {}", self.optname)
            }
            (OptLevel::Sctp, libc::SCTP_RTOINFO) => {
                let OptInput::SctpAssocId(assoc_id) = self.input else {
                    unreachable!()
                };

                // TODO: implement
                // based on default values for Debian 12/Linux 6.X
                Outcome::Success(SocketOption::SctpRtoInfo(SctpRtoInfo {
                    srto_assoc_id: assoc_id,
                    srto_initial: 3000,
                    srto_max: 60000,
                    srto_min: 1000,
                }))
            }
            (OptLevel::Sctp, SCTP_GET_LOCAL_ADDRS) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpGetLocalAddrs(Vec::new()))
            }
            (OptLevel::Sctp, libc::SCTP_INITMSG) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpInitMsg(SctpInitMsg {
                    sinit_num_ostreams: 10,
                    sinit_max_instreams: 10,
                    sinit_max_attempts: 8,
                    sinit_max_init_timeo: 60000,
                }))
            }
            (OptLevel::Sctp, libc::SCTP_NODELAY) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpNoDelay(true))
            }
            (OptLevel::Sctp, libc::SCTP_AUTOCLOSE) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpAutoClose(false))
            }
            (OptLevel::Sctp, libc::SCTP_SET_PEER_PRIMARY_ADDR) => Outcome::Error(Errno::EINVAL),
            (OptLevel::Sctp, libc::SCTP_PRIMARY_ADDR) => Outcome::Error(Errno::EINVAL),
            (OptLevel::Sctp, libc::SCTP_DISABLE_FRAGMENTS) => {
                Outcome::Success(SocketOption::SctpDisableFragments(false))
            }
            (OptLevel::Sctp, libc::SCTP_PEER_ADDR_PARAMS) => {
                let OptInput::SctpPeerAddrParams(assoc_id, addr) = self.input else {
                    unreachable!()
                };

                // TODO: implement
                Outcome::Success(SocketOption::SctpPeerAddrParams(SctpPeerAddrParams {
                    spp_assoc_id: assoc_id,
                    spp_address: addr,
                    spp_hbinterval: 30000,
                    spp_pathmaxrxt: 5,
                    spp_pathmtu: 1260,
                    spp_sackdelay: 200,
                    spp_flags: 1 | (1 << 3) | (1 << 5),
                    spp_ipv6_flowlabel: 0,
                    spp_dscp: 0,
                }))
            }
            (OptLevel::Sctp, libc::SCTP_DEFAULT_SEND_PARAM) => {
                // TODO: implement
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Sctp, libc::SCTP_EVENTS) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpEvents(SctpEventSubscribe {
                    sctp_data_io_event: 0,
                    sctp_association_event: 0,
                    sctp_address_event: 0,
                    sctp_send_failure_event: 0,
                    sctp_peer_error_event: 0,
                    sctp_shutdown_event: 0,
                    sctp_partial_delivery_event: 0,
                    sctp_adaptation_layer_event: 0,
                    sctp_authentication_event: 0,
                    sctp_sender_dry_event: 0,
                    sctp_stream_reset_event: 0,
                    sctp_assoc_reset_event: 0,
                    sctp_stream_change_event: 0,
                    sctp_send_failure_event_event: 0,
                }))
            }
            (OptLevel::Sctp, libc::SCTP_I_WANT_MAPPED_V4_ADDR) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpWantMappedV4Addr(false))
            }
            (OptLevel::Sctp, libc::SCTP_FRAGMENT_INTERLEAVE) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpFragmentInterleave(false))
            }
            (OptLevel::Sctp, libc::SCTP_MAXSEG) => {
                // TODO: implement
                Outcome::Success(SocketOption::SctpMaxSegment(0))
            }
            (OptLevel::Sctp, libc::SCTP_ASSOCINFO) => {
                let OptInput::SctpAssocId(assoc_id) = self.input else {
                    unreachable!()
                };

                // TODO: implement
                Outcome::Success(SocketOption::SctpAssocInfo(SctpAssocParams {
                    sasoc_assoc_id: assoc_id,
                    sasoc_asocmaxrxt: 10,
                    sasoc_peer_rwnd: 1,
                    sasoc_local_rwnd: 1,
                    sasoc_cookie_life: 60000,
                }))
            }
            (OptLevel::Sctp, libc::SCTP_STATUS) => {
                // TODO: implement
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Sctp, libc::SCTP_GET_PEER_ADDR_INFO) => {
                // TODO: implement
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Sctp, SCTP_GET_ASSOC_STATS) => {
                // TODO: implement
                Outcome::Error(Errno::EINVAL)
            }
            (OptLevel::Sctp, _) => {
                log::error!("unrecognized socket option for SOL_SCTP: {}", self.optname);
                panic!("unrecognized SOL_SCTP sockopt {}", self.optname)
            }
            (OptLevel::Tcp, libc::TCP_USER_TIMEOUT) => {
                // TODO: implement assigning (and enforcing) timeout on sockets
                Outcome::Success(SocketOption::TcpUserTimeout(Duration::from_millis(20000)))
            }
            (OptLevel::Tcp, libc::TCP_NODELAY) => {
                // TODO: implement assigning nodelay on sockets
                Outcome::Success(SocketOption::TcpNoDelay(true))
            }
            (OptLevel::Tcp, libc::TCP_MAXSEG) => {
                // TODO: implement assigning MSS on sockets
                Outcome::Success(SocketOption::TcpMss(1220))
            }
            (OptLevel::Tcp, _) => {
                log::error!("unrecognized socket option for SOL_TCP: {}", self.optname);
                panic!("unrecognized SOL_TCP sockopt {}", self.optname)
            }
        }
    }
}

pub struct SocketSetOptionEvent {
    descriptor_id: DescriptorId,
    option: SocketOption,
}

impl SocketSetOptionEvent {
    pub fn new(descriptor_id: DescriptorId, option: SocketOption) -> Self {
        Self {
            descriptor_id,
            option,
        }
    }
}

impl Event for SocketSetOptionEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        match &self.option {
            SocketOption::SocketIsListening(_)
            | SocketOption::SocketDomain(_)
            | SocketOption::SocketError(_)
            | SocketOption::SocketProtocol(_)
            | SocketOption::SctpGetLocalAddrs(_) => Outcome::Error(Errno::ENOPROTOOPT),
            // TODO: implement
            SocketOption::SocketDontRoute(_b) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketKeepalive(_b) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketLinger(_t) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketOobInline(_b) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketZeroCopy(_b) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketPriority(_u) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketRecvBuffer(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketSendLowWatermark(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketRecvLowWatermark(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SocketReuseAddr(_) => Outcome::Success(()),
            SocketOption::SocketReusePort(reuse) => {
                let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();

                match &mut socket_info.state {
                    SocketState::Connectionless(connectionless_socket) => {
                        connectionless_socket.reuse_port = *reuse
                    }
                    SocketState::Unassociated(unassociated_socket) => {
                        unassociated_socket.reuse_port = *reuse
                    }
                    _ => {
                        let transport_addr = get_or_assign_local(socket_id, state);
                        state
                            .global
                            .socket_locations
                            .get_mut(&transport_addr)
                            .unwrap()
                            .reuse_port = *reuse;
                    }
                }

                Outcome::Success(())
            }
            // TODO: implement
            SocketOption::IpOptions(_vec) => Outcome::Success(()),
            // TODO: implement, especially for binding rules
            SocketOption::Ipv6Only(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpRtoInfo(_info) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpInitMsg(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpNoDelay(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpAutoClose(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpDisableFragments(_) => todo!(),
            // TODO: implement
            SocketOption::SctpPeerAddrParams(_params) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpEvents(_subscribe) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpWantMappedV4Addr(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpFragmentInterleave(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpMaxSegment(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::SctpAssocInfo(_params) => Outcome::Success(()),
            // TODO: implement
            SocketOption::TcpUserTimeout(_duration) => Outcome::Success(()),
            // TODO: implement
            SocketOption::TcpNoDelay(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::TcpMss(_) => Outcome::Success(()),
        }
    }
}

/// Assigns a concrete local address to the socket pointed to by `socket_id`.
fn get_or_assign_local(socket_id: Rc<SocketId>, state: &mut FizzleState) -> TransportAddress {
    let socket_info = state.global.sockets.get(&socket_id).unwrap();
    let addr = socket_info.local_addr.clone();
    let protocol = socket_info.protocol;

    let reuse_port = match &socket_info.state {
        SocketState::Unassociated(u) => u.reuse_port,
        SocketState::Connectionless(c) => c.reuse_port,
        _ => false,
    };

    match &addr {
        // An assigned address has already had location info checked
        LocalAddress::Assigned(a) => TransportAddress {
            sockaddr: a.clone(),
            protocol: protocol,
        },
        LocalAddress::Ephemeral(family) => {
            let family = *family;
            let proto = protocol;

            // Check to see if the ephemeral address will bind
            loop {
                let addr = state.global.ephemeral_address(family, proto);

                let wildcard_bound = if let Some(wildcard) = addr.wildcard() {
                    state
                        .global
                        .socket_locations
                        .get(&wildcard)
                        .map_or(false, |a| {
                            (!a.reuse_port || !reuse_port) || !a.bound_sockets.is_empty()
                        })
                } else {
                    false
                };

                let Some(location_info) = state.global.socket_locations.get_mut(&addr) else {
                    let mut bound_sockets = Deque::new();
                    bound_sockets.push_back(socket_id);

                    state.global.socket_locations.insert(
                        addr.clone(),
                        TransportLocationInfo {
                            reuse_port,
                            bound_sockets,
                            pending: None,
                        },
                    );
                    break addr;
                };

                if (reuse_port && location_info.reuse_port)
                    || (!wildcard_bound && location_info.bound_sockets.is_empty())
                {
                    location_info.bound_sockets.push_back(socket_id).unwrap();
                    location_info.reuse_port |= reuse_port;
                    break addr;
                }

                log::warn!("ephemeral address assignment {} failed (already bound)--retrying with new address...", addr);
            }
        }
    }
}
