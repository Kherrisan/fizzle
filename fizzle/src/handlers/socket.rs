use std::{cmp, mem};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::os::fd::RawFd;
use crate::arena::{ArenaKey, Rc};
use crate::backend::{ConnectedBackend, ConnectingBackend, ConnectionlessBackend, PendingBackend, RegularConnected, ServerBackend, StandardFeedback};
use crate::constants::{FIZZLE_EPHEMERAL_PORT_END, FIZZLE_MIN_CONNECTIONLESS, FIZZLE_MAX_REUSEPORT, FIZZLE_SOMAXCONN};
use crate::state::{FizzleSingleton, FizzleState};

use fizzle_common::io::{AddressFamily, SockAddr, SocketType, TransportAddress, TransportProtocol};
use fizzle_common::storage::Buffer;
use heapless::{Deque, Entry};
pub use private::SocketId;

use super::buffer::BufferId;
use super::descriptor::{DescriptorError, DescriptorId, DescriptorInfo, FdResource};
use super::fuzz_endpoint::FuzzEndpointInfo;
use super::polled::{PolledId, PolledInfo};
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

#[derive(Debug)]
pub enum SocketState {
    Connectionless(ConnectionlessSocket),
    Unassociated(UnassociatedSocket),
    Server(ServerSocket),
    PendingConnection(PendingSocket),
    Connecting(ConnectingSocket),
    Connected(ConnectedSocket),
    //    Error state?
}

#[derive(Debug)]
pub struct ConnectionlessSocket {
    pub reuse_port: bool,
    pub backend: ConnectionlessBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: Option<TransportAddress>,
}

#[derive(Debug)]
pub struct UnassociatedSocket {
    pub family: AddressFamily,
    pub socktype: SocketType,
    pub protocol: TransportProtocol,
    pub local_addr: Option<TransportAddress>,
    pub reuse_port: bool,
}

impl UnassociatedSocket {
    pub fn new(family: AddressFamily, socktype: SocketType, protocol: TransportProtocol) -> Self {
        Self {
            family,
            socktype,
            protocol,
            local_addr: None,
            reuse_port: false,
        }
    }
}

#[derive(Debug)]
pub struct ServerSocket {
    pub backend: ServerBackend,
    pub local_addr: TransportAddress,
    pub connecting: heapless::Deque<Rc<SocketId>, FIZZLE_SOMAXCONN>,
    pub ready_to_connect: Rc<PolledId>,
}

#[derive(Clone, Debug)]
pub struct PendingSocket {
    pub backend: PendingBackend,
    pub next_pending: Option<Rc<SocketId>>,
    pub local_addr: TransportAddress,
    pub rem_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectingSocket {
    pub backend: ConnectingBackend,
    pub connect_polled: Rc<PolledId>,
    pub local_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectedSocket {
    pub backend: ConnectedBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: TransportAddress,
    pub peer_closed: bool,
}

impl ArenaKey for SocketId {
    type Value = SocketState;
}

impl Rc<SocketId> {
    pub fn next_ephemeral_port(state: &mut FizzleState) -> u16 {
        let port = state.global.next_ephemeral_port;
        if state.global.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
            state.global.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_END;
        } else {
            state.global.next_ephemeral_port += 1;
        }

        port
    }

    fn wildcard_addr(transport_addr: &TransportAddress) -> Option<TransportAddress> {
        match &transport_addr.sockaddr {
            SockAddr::Ipv4(v4_addr) => Some(TransportAddress {
                sockaddr: SockAddr::Ipv4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, v4_addr.port())),
                protocol: transport_addr.protocol,
            }),
            SockAddr::Ipv6(v6_addr) => Some(TransportAddress {
                sockaddr: SockAddr::Ipv6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, v6_addr.port(), v6_addr.flowinfo(), v6_addr.scope_id())),
                protocol: transport_addr.protocol,
            }),
            SockAddr::Unix(_) => None,
        }
    }

    fn wildcard_is_bound(state: &mut FizzleState, address: &TransportAddress) -> bool {
        let Some(wildcard_addr) = Self::wildcard_addr(address) else {
            return false
        };

        match state.global.socket_locations.get(&wildcard_addr) {
            Some(wildcard) if !wildcard.bound_sockets.is_empty() => true,
            _ => false,
        }
    }

    pub fn bind(&self, ctx: &mut FizzleSingleton, mut sockaddr: SockAddr) -> Result<(), SocketError> {
        let mut state = ctx.acquire();

        // If port is 0, bind to an ephemerally-chosen port
        match &mut sockaddr {
            SockAddr::Ipv4(v4_addr) if v4_addr.port() == 0 => {
                v4_addr.set_port(Self::next_ephemeral_port(&mut state));
            }
            SockAddr::Ipv6(v6_addr) if v6_addr.port() == 0 => {
                v6_addr.set_port(Self::next_ephemeral_port(&mut state));
            }
            _ => (),
        }

        // TODO: support AF_UNSPEC?
        let (old_addr, new_addr, reuse_port) = match state.global.sockets.get_mut(self).unwrap() {
            SocketState::Connectionless(conn) => {
                let bind_address = TransportAddress {
                    sockaddr,
                    protocol: conn.local_addr.protocol,
                };

                let old_addr = mem::replace(&mut conn.local_addr, bind_address.clone());
                (Some(old_addr), bind_address, conn.reuse_port)
            }
            SocketState::Unassociated(unassoc) => {
                let bind_address = TransportAddress {
                    sockaddr,
                    protocol: unassoc.protocol,
                };

                let old_addr = mem::replace(&mut unassoc.local_addr, Some(bind_address.clone()));
                (old_addr, bind_address, unassoc.reuse_port)
            }
            SocketState::Server(_) => {
                log::error!("rebinding of listening socket unsupported by Fizzle");
                return Err(SocketError::InvalidState)
            }
            SocketState::PendingConnection(_) => unreachable!(), // Pending connections don't have fd handles
            SocketState::Connecting(_) => {
                log::error!("rebinding of connecting socket unsupported by Fizzle");
                return Err(SocketError::InvalidState)
            }
            SocketState::Connected(_) => {
                log::error!("rebinding of connected socket unsupported by Fizzle");
                return Err(SocketError::InvalidState)
            }
        };

        let wildcard_is_bound = Self::wildcard_is_bound(&mut state, &new_addr);

        match state.global.socket_locations.entry(new_addr) {
            Entry::Occupied(mut o) => {
                let location_info = o.get_mut();
                if !wildcard_is_bound && (location_info.bound_sockets.is_empty() || (reuse_port && location_info.reuse_port)) {
                    location_info.bound_sockets.push_back(self.clone()).unwrap();
                    location_info.reuse_port = reuse_port
                } else {
                    log::warn!("socket attempted to bind to bound address {}", o.key());

                    match state.global.sockets.get_mut(self).unwrap() {
                        SocketState::Connectionless(conn) => conn.local_addr = old_addr.unwrap(),
                        SocketState::Unassociated(unassoc) => unassoc.local_addr = old_addr,
                        _ => unreachable!(),
                    }

                    return Err(SocketError::AddressInUse)
                }
            }
            Entry::Vacant(v) => {
                let mut bound_sockets = Deque::new();
                bound_sockets.push_back(self.clone()).unwrap();

                v.insert(TransportLocationInfo {
                    reuse_port,
                    bound_sockets,
                    pending: None,
                }).unwrap();
            }
        }

        if let Some(address) = old_addr {
            match state.global.socket_locations.entry(address) {
                Entry::Vacant(v) => {
                    log::warn!("old socket address {} not found in Fizzle state!", v.key());
                }
                Entry::Occupied(mut o) => {
                    if o.get().bound_sockets.len() == 1 && o.get().pending.is_none() {
                        o.remove();
                    } else {
                        let bound_queue_len = o.get().bound_sockets.len();
                        for _ in 0..bound_queue_len {
                            let bound_sockets = &mut o.get_mut().bound_sockets;
                            let socket_id = bound_sockets.pop_front().unwrap();
                            if &socket_id == self {
                                return Ok(()) // bound socket has been removed
                            }
                            bound_sockets.push_back(socket_id).unwrap();
                        }
                        
                        unreachable!()
                    }
                }
            }
        }

        Ok(())
    }

    pub fn listen(&self, ctx: &mut FizzleSingleton) -> Result<(), SocketError> {
        let mut state = ctx.acquire();

        let SocketState::Unassociated(socket_info) = state.global.sockets.get_mut(self).unwrap() else {
            log::error!("calling listen() on a connected or listening socket unsupported by Fizzle");
            return Err(SocketError::InvalidState)
        };

        let reuse_port = socket_info.reuse_port;

        let local_addr = match &socket_info.local_addr {
            Some(addr) => addr.clone(),
            None => {
                log::warn!("listen() called on socket without prior bind()");

                let family = socket_info.family;
                let protocol = socket_info.protocol;

                let mut addr = state.global.ephemeral_address(family, protocol);
                while state.global.socket_locations.contains_key(&addr) || Self::wildcard_addr(&addr).map(|a| state.global.socket_locations.contains_key(&a)) == Some(true) {
                    addr = state.global.ephemeral_address(family, protocol);
                    // BUG: infinite loop if literally every ephemeral port is bound (which is impossible given current const limits)
                }

                let mut bound_sockets = heapless::Deque::new();
                bound_sockets.push_back(self.clone()).unwrap();

                state.global.socket_locations.insert(addr.clone(), TransportLocationInfo {
                    bound_sockets,
                    pending: None,
                    reuse_port,
                }).unwrap();

                addr
            }
        };

        // Allocate server context and set up polling
        let ready_to_connect = state.global.polled_events.allocate(PolledInfo::new()).unwrap();

        if state.global.socket_locations.get_mut(&local_addr).unwrap().pending.is_some() {
            state.raise_polled(&ready_to_connect);
        }

        *state.global.sockets.get_mut(self).unwrap() = SocketState::Server(ServerSocket {
            backend: ServerBackend::Peered(()),
            local_addr,
            connecting: heapless::Deque::new(),
            ready_to_connect,
        });

        Ok(())
    }

    pub fn connect(&self, ctx: &mut FizzleSingleton, rem_addr: SockAddr, nonblocking: bool) -> Result<(), SocketError> {
        let mut state = ctx.acquire();

        match state.global.sockets.get(self).unwrap() {
            SocketState::Unassociated(sock) => {
                let protocol = sock.protocol;
                let family = sock.family;
                let local_addr = match &sock.local_addr {
                    Some(addr) => addr.clone(),
                    None => state.global.ephemeral_address(family, protocol),
                };

                let transport_addr = TransportAddress {
                    sockaddr: rem_addr,
                    protocol,
                };

                let server_socket_id: Rc<SocketId>;

                if let Some(socket_id) = state.global.socket_locations.get(&transport_addr).and_then(|l| l.bound_sockets.front()) {
                    // The exact address is bound
                    server_socket_id = socket_id.clone();
                } else if let Some(socket_id) = Self::wildcard_addr(&transport_addr).and_then(|t| state.global.socket_locations.get(&t)).and_then(|l| l.bound_sockets.front()) {
                    // The wildcard address is bound
                    server_socket_id = socket_id.clone();
                } else {
                    // No socket is bound to the given address...
                    log::warn!("connect() on address {} failed--no server listening", &transport_addr);
                    return Err(SocketError::AddressNotListening)
                }

                let SocketState::Server(server_info) = state.global.sockets.get_mut(&server_socket_id).unwrap() else {
                    log::error!("inconsistent Fizzle state--socket bound to connect() address {} not in listening state", &transport_addr);
                    return Err(SocketError::AddressNotListening)
                };

                let server_backend = server_info.backend.clone();
                let connecting = &mut server_info.connecting;

                let connected_backend = match server_backend {
                    ServerBackend::Passthrough => unreachable!(),
                    ServerBackend::Peered(()) => {
                        let Ok(_) = connecting.push_back(self.clone()) else {
                            return Err(SocketError::ConnectionQueueFull)
                        };

                        let server_poll = server_info.ready_to_connect.clone();
                        state.raise_polled(&server_poll);

                        let client_poll = state.global.polled_events.allocate(PolledInfo::new()).unwrap();
                        *state.global.sockets.get_mut(self).unwrap() = SocketState::Connecting(ConnectingSocket {
                            backend: ConnectingBackend::Peered(()),
                            connect_polled: client_poll.clone(),
                            local_addr,
                        });

                        // The server side sets this backend to conected, so we don't need to here.
                        if nonblocking {
                            return Err(SocketError::ConnectInProgress)
                        } else {
                            drop(state);
                            ctx.poll_until_ready(client_poll);
                            return Ok(())
                        }
                    }
                    ServerBackend::Plugin(plugin_id) => {
                        // Create new plugin
                        let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                        let endpoint = plugin_info.endpoint.clone();
                        let module_id = plugin_info.module_id.clone();
                        let connect_plugin_id = state.global.add_plugin(endpoint, module_id);
                        ConnectedBackend::Plugin(connect_plugin_id)
                    },
                    ServerBackend::Sink => ConnectedBackend::Sink,
                    ServerBackend::Fuzz(_) => ConnectedBackend::Fuzz(state.global.add_fuzz_endpoint()),
                    ServerBackend::NullSink => ConnectedBackend::NullSink,
                    ServerBackend::Feedback(()) => ConnectedBackend::Feedback(StandardFeedback {
                            buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                            read_polled: state.global.polled_events.allocate(PolledInfo::new()).unwrap(),
                            write_polled: state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap(),
                    })
                };

                *state.global.sockets.get_mut(self).unwrap() = SocketState::Connected(ConnectedSocket {
                    backend: connected_backend,
                    local_addr,
                    rem_addr: transport_addr,
                    peer_closed: false,
                });

                Ok(())
            },
            SocketState::Server(_) => {
                log::error!("connect() called on listening socket--unsupported by Fizzle");
                Err(SocketError::InvalidState)
            },
            SocketState::PendingConnection(_) => unreachable!(),
            SocketState::Connecting(_) => if nonblocking {
                Err(SocketError::ConnectAlreadyStarted)
            } else {
                drop(state);
                ctx.yield_thread();
                Ok(())
            }
            SocketState::Connected(_) => Err(SocketError::ConnectAlreadyCompleted),
            SocketState::Connectionless(_) => unreachable!(),
        }
    }

    pub fn accept(&self, ctx: &mut FizzleSingleton, nonblocking: bool, close_on_exec: bool) -> Result<(RawFd, SockAddr), SocketError> {
        let mut state = ctx.acquire();

        let SocketState::Server(server_info) = state.global.sockets.get_mut(self).unwrap() else {
            log::error!("accept() called on non-listening socket");
            return Err(SocketError::InvalidState)
        };

        let has_connecting = !server_info.connecting.is_empty();
        let server_address = server_info.local_addr.clone();
        let server_backend = server_info.backend.clone();
        let ready_to_connect = server_info.ready_to_connect.clone();

        // Variables to be determined
        let client_address: TransportAddress;
        let client_id: Rc<SocketId>;
        let client_backend: ConnectingBackend;

        let bound_info = state.global.socket_locations.get(&server_address).unwrap();
        if let Some(PendingInfo { client, poll }) = bound_info.pending.clone() {
            let SocketState::PendingConnection(pending_info) = state.global.sockets.get_mut(&client).unwrap() else {
                unreachable!()
            };

            client_address = pending_info.local_addr.clone();
            client_id = client;
            client_backend = pending_info.backend.clone();

            // Update the linked list of pending clients
            match pending_info.next_pending.clone() {
                Some(pending_id) => state.global.socket_locations.get_mut(&server_address).unwrap().pending.as_mut().unwrap().client = pending_id,
                None => {
                    state.global.socket_locations.get_mut(&server_address).unwrap().pending = None;

                    if !has_connecting {
                        state.lower_polled(&ready_to_connect);
                    }
                }
            }

            state.raise_polled(&poll);
            drop(state);

        } else {
            let SocketState::Server(server_info) = state.global.sockets.get_mut(self).unwrap() else {
                unreachable!()
            };

            if let Some(connecting_id) = server_info.connecting.pop_front() {
                if server_info.connecting.len() == 1 { // TODO: why isn't this is_empty()???
                    state.lower_polled(&ready_to_connect);
                }

                let SocketState::Connecting(connecting_info) = state.global.sockets.get(&connecting_id).unwrap() else {
                    unreachable!()
                };

                client_address = connecting_info.local_addr.clone();
                client_id = connecting_id;
                client_backend = connecting_info.backend.clone();

                let connect_polled = connecting_info.connect_polled.clone();
                state.raise_polled(&connect_polled);
                drop(state);

            } else if nonblocking {
                return Err(SocketError::AcceptPending)

            } else {
                drop(state);
                ctx.poll_until_ready(ready_to_connect.clone());
                let mut state = ctx.acquire();

                // Now there's a connected socket ready
                let SocketState::Server(server_info) = state.global.sockets.get_mut(self).unwrap() else {
                    unreachable!()
                };
                let connecting_cnt = server_info.connecting.len();

                let connecting_id = server_info.connecting.pop_front().unwrap();
                let SocketState::Connecting(connecting_info) = state.global.sockets.get(&connecting_id).unwrap() else {
                    unreachable!()
                };

                client_address = connecting_info.local_addr.clone();
                client_id = connecting_id;
                client_backend = connecting_info.backend.clone();

                if connecting_cnt == 1 { // TODO: shouldn't this be is_empty()???
                    state.lower_polled(&ready_to_connect);
                }
                drop(state);
            }
        }

        let mut state = ctx.acquire();

        let new_connect_backend = match client_backend {
            ConnectingBackend::Passthrough => unreachable!(),
            ConnectingBackend::Peered(()) => ConnectedBackend::Peered(RegularConnected {
                peer: Some(client_id.clone()),
                recv_buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                read_polled: state.global.polled_events.allocate(PolledInfo::new()).unwrap(),
                write_polled: state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap(),
            }),
            ConnectingBackend::Feedback(()) => ConnectedBackend::Feedback(StandardFeedback {
                buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                read_polled: state.global.polled_events.allocate(PolledInfo::new()).unwrap(),
                write_polled: state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap(),
            }),
            ConnectingBackend::Plugin(plugin_id) => {
                let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                let endpoint = plugin_info.endpoint.clone();
                let module_id = plugin_info.module_id.clone();
                let connect_plugin_id = state.global.add_plugin(endpoint, module_id);
                ConnectedBackend::Plugin(connect_plugin_id)
            },
            ConnectingBackend::Sink => ConnectedBackend::Sink,
            ConnectingBackend::NullSink => ConnectedBackend::NullSink,
            ConnectingBackend::Fuzz(endpoint) => ConnectedBackend::Fuzz(endpoint),
        };

        let client_addr = client_address.addr().clone();

        let socket_id = if let ConnectedBackend::Peered(_) = new_connect_backend {
            let accept_backend = match server_backend {
                ServerBackend::Peered(_) => ConnectedBackend::Peered(RegularConnected {
                    peer: Some(client_id.clone()),
                    recv_buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
                    read_polled: state.global.polled_events.allocate(PolledInfo::new()).unwrap(),
                    write_polled: state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap(),
                }),
                _ => unreachable!(),
            };

            *state.global.sockets.get_mut(&client_id).unwrap() =
                SocketState::Connected(ConnectedSocket {
                    local_addr: client_address.clone(),
                    rem_addr: server_address.clone(),
                    backend: new_connect_backend,
                    peer_closed: false,
                });

            state.global
                .sockets
                .allocate(SocketState::Connected(ConnectedSocket {
                    local_addr: server_address,
                    rem_addr: client_address,
                    backend: accept_backend,
                    peer_closed: false,
                })).unwrap()
        } else {
            // The connecting socket was emulated in some way (`fuzz`, `sink` or the like).
            // Convert the connecting socket into the accepted socket--we don't need two peered sockets.
            *state.global.sockets.get_mut(&client_id).unwrap() =
                SocketState::Connected(ConnectedSocket {
                    local_addr: server_address,
                    rem_addr: client_address,
                    backend: new_connect_backend,
                    peer_closed: false,
                });

            client_id
        };

        // TODO: bind client address here

        let new_fd = crate::alias_fd_create();
        // The two sockets are now joined--add a file descriptor to the accepted socket
        state.local.fds.allocate_with_key(
            DescriptorId::from_raw_fd(new_fd),
            DescriptorInfo {
                close_on_exec,
                is_passthrough: false,
                nonblocking,
                resource: FdResource::Socket(socket_id),
            },
        ).unwrap();

        Ok((new_fd, client_addr))
    }

    pub fn socket_name(&self, ctx: &mut FizzleSingleton) -> Result<SockAddr, SocketError> {
        let state = ctx.acquire();

        match state.global.sockets.get(self).unwrap() {
            SocketState::Connectionless(conn) => Ok(conn.local_addr.addr().clone()),
            SocketState::Unassociated(conn) => Ok(conn.local_addr.clone().map(|a| a.addr().clone()).unwrap()),
            SocketState::Server(conn) => Ok(conn.local_addr.addr().clone()),
            SocketState::PendingConnection(_) => unreachable!(),
            SocketState::Connecting(conn) => Ok(conn.local_addr.addr().clone()),
            SocketState::Connected(conn) => Ok(conn.local_addr.addr().clone()),
        }
    }

    pub fn read(&self, ctx: &mut FizzleSingleton, msg: &mut MsgHdrOut, nonblocking: bool) -> Result<usize, SocketError> {
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
                    _ => unreachable!()
                }

                if let Some(read_polled) = read_polled.as_ref() {
                    let polled_is_ready = state.polled_is_ready(&read_polled);
                    drop(state);

                    if !polled_is_ready {
                        if nonblocking {
                            return Err(SocketError::WouldBlock)
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
            },
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
                                return Ok(0)
                            }
                            if nonblocking {
                                return Err(SocketError::WouldBlock)
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
                            return Ok(0) // The connection was shut down while we were polling on it
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
                                return Ok(0)
                            }
                            if nonblocking {
                                return Err(SocketError::WouldBlock)
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
                            return Ok(0)
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
                                return Ok(0)
                            }
                            if nonblocking {
                                return Err(SocketError::WouldBlock)
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
                            return Ok(0)
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

                        return Ok(total_read)
                    }
                    ConnectedBackend::Fuzz(fuzz_endpoint_id) => {
                        let fuzz_endpoint_id = fuzz_endpoint_id.clone();
                        let FuzzEndpointInfo { mut read_idx, read_polled } = state.global.fuzz_endpoints.get(&fuzz_endpoint_id).unwrap().clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            if peer_closed {
                                return Ok(0)
                            }

                            if nonblocking {
                                return Err(SocketError::WouldBlock)
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

                        msg.set_ancillary_len(0);
                        let addrlen = rem_sockaddr.encode(msg.addr_bytes()) as u32;
                        msg.set_addrlen(addrlen);

                        return Ok(total_read)
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
            },
            _ => Err(SocketError::InvalidState)
        }
    }

    pub fn write(&self, ctx: &mut FizzleSingleton, msg: &impl MsgHdr, nonblocking: bool) -> Result<usize, SocketError> {
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

                        if let Some(rem_socket_id) = state.global.socket_locations.get_mut(&dst_addr).and_then(|i| i.bound_sockets.front().cloned()) {
                            socket_id = rem_socket_id;

                        } else if let Some(rem_socket_id) = Self::wildcard_addr(&dst_addr).and_then(|a| state.global.socket_locations.get_mut(&a)).and_then(|i| i.bound_sockets.front().cloned()) {
                            socket_id = rem_socket_id;

                        } else {
                            log::error!("packet send to location {} that had no listening ports--packet silently dropped", dst_addr);
                            return Ok(msg.vdata().iter().map(|v| v.data().len()).sum())
                        }

                        let SocketState::Connectionless(conn) = state.global.sockets.get(&socket_id).unwrap() else {
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
                    ConnectionlessBackend::Sink | ConnectionlessBackend::NullSink | ConnectionlessBackend::Fuzz(_) => return Ok(msg.vdata().iter().map(|v| v.data().len()).sum()),
                }

                let polled_is_ready = state.polled_is_ready(&write_polled);
                drop(state);

                if !polled_is_ready {
                    if nonblocking {
                        return Err(SocketError::WouldBlock)
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
                    return Ok(0)
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
                                return Err(SocketError::WouldBlock)
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let state = ctx.acquire();

                        let Some(SocketState::Connected(ConnectedSocket {
                            peer_closed: false,
                            backend: ConnectedBackend::Peered(RegularConnected { peer: Some(_), .. }),
                            ..
                        })) = state.global.sockets.get(self)
                        else {
                            return Ok(0) // The connection was shut down while we were polling on it
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
                                return Err(SocketError::WouldBlock)
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
                            return Ok(0)
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
                                return Err(SocketError::WouldBlock)
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
                            return Ok(0)
                        };

                        drop(state);
                    }
                    ConnectedBackend::Sink | ConnectedBackend::NullSink | ConnectedBackend::Fuzz(_) => return Ok(msg.vdata().iter().map(|v| v.data().len()).sum()),
                }

                let mut state = ctx.acquire();

                let write_buffer = state.global.buffers.get_mut(&buffer_id).unwrap();
                let mut total_written = 0;
                for iovec in msg.vdata() {
                    if write_buffer.is_full() {
                        break
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
            },
            _ => Err(SocketError::InvalidState)
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
                return *val as libc::c_int
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
            Err(SocketError::AddressNotListening | SocketError::ConnectionQueueFull) => "-1 (ECONNREFUSED)",
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
