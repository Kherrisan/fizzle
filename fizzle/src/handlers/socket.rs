use fizzle_common::io::*;
use hashbrown::hash_map::Entry;

use std::cell::RefCell;
use std::collections::{LinkedList, VecDeque};
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::rc::{Rc, Weak};
use std::time::Duration;
use std::{cmp, mem, ptr, slice};

use crate::backend::{
    ConnectedBackend, ConnectingBackend, ConnectionlessBackend, PendingBackend, RegularConnected,
    RegularConnectionless, ServerBackend, StandardFeedback,
};
use crate::constants::{FIZZLE_BUFFER_LENGTH, FIZZLE_EPHEMERAL_PORT_END, FIZZLE_SOMAXCONN};
use crate::errno::Errno;
use crate::scheduler::{fizzle_alloc, Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::{GlobalDeque, GlobalHeap, GlobalList, GlobalRc};

use super::descriptor::*;
use super::polled::PolledInfo;
use super::poller::PollerInfo;

fn get_or_assign_local(
    socket_info: &mut GlobalRc<SocketInfo>,
    state: &mut FizzleState,
) -> TransportAddress {
    let addr = socket_info.borrow().local_addr.clone();
    let protocol = socket_info.borrow().protocol;

    let mut reuse_port = match &socket_info.borrow().state {
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
            let family = family.clone();
            let proto = protocol;

            // Check to see if the ephemeral address will bind
            loop {
                let addr = state.global.ephemeral_address(family, proto);

                if addr.addr() == &SockAddr::Unix(SocketAddrUnix::Unnamed) {
                    reuse_port = true;
                }

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
                    let mut bound_sockets = VecDeque::new_in(fizzle_alloc());
                    bound_sockets.push_back(socket_info.clone());

                    state.global.socket_locations.insert(
                        addr.clone(),
                        TransportLocationInfo {
                            reuse_port,
                            bound_sockets,
                            pending: LinkedList::new_in(fizzle_alloc()),
                        },
                    );
                    break addr;
                };

                if (reuse_port && location_info.reuse_port)
                    || (!wildcard_bound && location_info.bound_sockets.is_empty())
                {
                    location_info.bound_sockets.push_back(socket_info.clone());
                    location_info.reuse_port |= reuse_port;
                    break addr;
                }

                log::warn!("ephemeral address assignment {} failed (already bound)--retrying with new address...", addr);
            }
        }
    }
}

pub struct TransportLocationInfo {
    pub reuse_port: bool,
    pub bound_sockets: GlobalDeque<GlobalRc<SocketInfo>>,
    pub pending: GlobalList<GlobalRc<SocketInfo>>,
}

impl Debug for TransportLocationInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportLocationInfo")
            .field("reuse_port", &self.reuse_port)
            .field("bound_sockets", &"<opaque>")
            .field("pending", &"<opaque>")
            .finish()
    }
}

impl TransportLocationInfo {
    pub fn next_bound(&mut self) -> Option<GlobalRc<SocketInfo>> {
        let sock = self.bound_sockets.pop_front()?;
        self.bound_sockets.push_back(sock.clone());
        Some(sock)
    }
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

pub struct SocketOptions {
    pub tcp_user_timeout: Option<Duration>,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            tcp_user_timeout: None,
        }
    }
}

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
    pub options: SocketOptions,
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
            options: Default::default(),
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

pub enum SocketState {
    Connectionless(ConnectionlessSocket),
    Unassociated(UnassociatedSocket),
    Server(ServerSocket),
    PendingConnection(PendingSocket),
    Connecting(ConnectingSocket),
    Connected(ConnectedSocket),
}

pub struct ConnectionlessSocket {
    pub backend: ConnectionlessBackend,
    pub rem_addr: Option<TransportAddress>,
    pub reuse_port: bool,
}

impl ConnectionlessSocket {
    /// Returns the `Polled` instance used to notify of read data available for this socket.
    pub fn read_polled(&self) -> Option<GlobalRc<PolledInfo>> {
        match &self.backend {
            ConnectionlessBackend::Passthrough => unimplemented!(),
            ConnectionlessBackend::Peered(p) => Some(p.read_polled.clone()),
            ConnectionlessBackend::Feedback(_) | ConnectionlessBackend::Plugin(_) | ConnectionlessBackend::Sink | ConnectionlessBackend::NullSink | ConnectionlessBackend::Fuzz(_) => unreachable!(),
        }
    }

    /// Returns the socket currently connected to.
    pub fn dst_socket(&self, state: &mut FizzleState) -> Option<GlobalRc<SocketInfo>> {
        match &self.backend {
            ConnectionlessBackend::Peered(_) => {
                let Some(addr) = &self.rem_addr else {
                    return None;
                };

                let Some(loc_info) = state.global.socket_locations.get_mut(addr) else {
                    return None;
                };

                let Some(peer_info) = loc_info.bound_sockets.front().cloned() else {
                    return None;
                };

                loc_info.bound_sockets.push_back(peer_info.clone());

                Some(peer_info)
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct UnassociatedSocket {
    reuse_port: bool,
}

pub struct ServerSocket {
    pub backend: ServerBackend,
    pub connecting: GlobalList<GlobalRc<SocketInfo>>,
    pub ready_to_connect: GlobalRc<PolledInfo>,
}

#[derive(Clone)]
pub struct PendingSocket {
    pub backend: PendingBackend,
    pub rem_addr: TransportAddress,
}

pub struct ConnectingSocket {
    pub backend: ConnectingBackend,
    pub connect_polled: GlobalRc<PolledInfo>,
}

pub struct ConnectedSocket {
    pub backend: ConnectedBackend,
    pub rem_addr: TransportAddress,
    pub peer_closed: bool,
}

impl ConnectedSocket {
    pub fn read_polled(&self) -> Option<GlobalRc<PolledInfo>> {
        match &self.backend {
            ConnectedBackend::Passthrough => unimplemented!(),
            ConnectedBackend::Peered(p) => Some(p.read_polled.clone()),
            ConnectedBackend::Feedback(f) => Some(f.read_polled.clone()),
            ConnectedBackend::Plugin(p) => Some(p.borrow().read_polled.clone()),
            ConnectedBackend::Sink => None,
            ConnectedBackend::NullSink => None,
            ConnectedBackend::Fuzz(f) => Some(f.borrow().read_polled.clone()),
        }
    }

    pub fn write_polled(&self) -> Option<GlobalRc<PolledInfo>> {
        match &self.backend {
            ConnectedBackend::Passthrough => unimplemented!(),
            ConnectedBackend::Peered(p) => {
                let peer = p.peer.upgrade()?;
                let peer_ref = peer.borrow();
                match &peer_ref.state {
                    SocketState::Connected(c) => match &c.backend {
                        ConnectedBackend::Peered(p) => Some(p.write_polled.clone()),
                        ConnectedBackend::Plugin(p) => Some(p.borrow().write_polled.clone()),
                        _ => unreachable!(),
                    },
                    _ => unreachable!(),
                }
            }
            ConnectedBackend::Feedback(f) => Some(f.write_polled.clone()),
            ConnectedBackend::Plugin(p) => Some(p.borrow().write_polled.clone()),
            ConnectedBackend::Sink => None,
            ConnectedBackend::NullSink => None,
            ConnectedBackend::Fuzz(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionlessMessage {
    pub source: SockAddr,
    pub ancillary: Vec<u8, GlobalHeap>,
    pub data: Vec<u8, GlobalHeap>,
}

pub struct SocketCreateEvent {
    pub domain: AddressFamily,
    pub socket_type: SocketType,
    pub protocol: TransportProtocol,
    pub nonblocking: bool,
    pub cloexec: bool,
}

impl Event for SocketCreateEvent {
    type Success = Descriptor;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let fd = Descriptor::from_raw_fd(crate::create_descriptor());

        let socket_info = Rc::new_in(
            RefCell::new(SocketInfo {
                fd_count: 1,
                socktype: self.socket_type,
                protocol: self.protocol,
                local_addr: LocalAddress::Ephemeral(self.domain),
                options: Default::default(),
                state: match self.socket_type {
                    SocketType::SeqPacket | SocketType::Stream => {
                        SocketState::Unassociated(UnassociatedSocket { reuse_port: false })
                    }
                    SocketType::Datagram => {
                        let read_polled = Rc::new_in(
                            RefCell::new(PolledInfo {
                                pollers: Vec::new_in(fizzle_alloc()),
                                event_raised: false,
                            }),
                            fizzle_alloc(),
                        );

                        let write_polled = Rc::new_in(
                            RefCell::new(PolledInfo {
                                pollers: Vec::new_in(fizzle_alloc()),
                                event_raised: false,
                            }),
                            fizzle_alloc(),
                        );

                        SocketState::Connectionless(ConnectionlessSocket {
                            reuse_port: false,
                            backend: ConnectionlessBackend::Peered(RegularConnectionless {
                                recv_buf: LinkedList::new_in(fizzle_alloc()),
                                read_polled,
                                write_polled,
                            }),
                            rem_addr: None,
                        })
                    }
                    SocketType::Raw if self.domain == AddressFamily::Netlink => {
                        SocketState::Connectionless(ConnectionlessSocket {
                            reuse_port: false,
                            backend: ConnectionlessBackend::Passthrough,
                            rem_addr: None,
                        })
                    }
                    SocketType::Raw => unimplemented!(),
                },
            }),
            fizzle_alloc(),
        );

        state.local.fds.insert(
            fd,
            DescriptorInfo {
                close_on_exec: self.cloexec,
                nonblocking: self.nonblocking,
                is_passthrough: self.domain == AddressFamily::Netlink, // TODO: implement Netlink routines
                is_random: false,
                resource: FdResource::Socket(socket_info),
            },
        );

        Outcome::Success(fd)
    }
}

pub struct NetlinkCreateEvent {
    pub fd: libc::c_int,
}

impl Event for NetlinkCreateEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state.local.fds.insert(
            Descriptor::from_raw_fd(self.fd),
            DescriptorInfo {
                close_on_exec: false,
                nonblocking: false,
                is_passthrough: true,
                is_random: false,
                resource: FdResource::Opaque,
            },
        );

        Outcome::Success(())
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
    type Success = (Descriptor, Descriptor);
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let addr1 = state.global.ephemeral_address(self.domain, self.protocol);
        let fd1 = Descriptor::from_raw_fd(crate::create_descriptor());

        let read_polled1 = Rc::new_in(
            RefCell::new(PolledInfo {
                pollers: Vec::new_in(fizzle_alloc()),
                event_raised: false,
            }),
            fizzle_alloc(),
        );

        let write_polled1 = Rc::new_in(
            RefCell::new(PolledInfo {
                pollers: Vec::new_in(fizzle_alloc()),
                event_raised: false,
            }),
            fizzle_alloc(),
        );

        let addr2 = state.global.ephemeral_address(self.domain, self.protocol);
        let fd2 = Descriptor::from_raw_fd(crate::create_descriptor());

        let read_polled2 = Rc::new_in(
            RefCell::new(PolledInfo {
                pollers: Vec::new_in(fizzle_alloc()),
                event_raised: false,
            }),
            fizzle_alloc(),
        );

        let write_polled2 = Rc::new_in(
            RefCell::new(PolledInfo {
                pollers: Vec::new_in(fizzle_alloc()),
                event_raised: false,
            }),
            fizzle_alloc(),
        );

        let reuse_port = addr1.addr() == &SockAddr::Unix(SocketAddrUnix::Unnamed);

        let socket1 = Rc::new_in(
            RefCell::new(SocketInfo {
                fd_count: 1,
                socktype: self.socket_type,
                protocol: self.protocol,
                local_addr: LocalAddress::Assigned(addr1.addr().clone()),
                options: Default::default(),
                state: match self.socket_type {
                    SocketType::SeqPacket | SocketType::Stream => {
                        SocketState::Connected(ConnectedSocket {
                            backend: ConnectedBackend::Peered(RegularConnected {
                                peer: Weak::new_in(fizzle_alloc()),
                                recv_buf: LinkedList::new_in(fizzle_alloc()),
                                read_idx: 0,
                                read_polled: read_polled1,
                                write_polled: write_polled1,
                            }),
                            rem_addr: addr2.clone(),
                            peer_closed: false,
                        })
                    }
                    SocketType::Datagram => SocketState::Connectionless(ConnectionlessSocket {
                        reuse_port,
                        backend: ConnectionlessBackend::Peered(RegularConnectionless {
                            recv_buf: LinkedList::new_in(fizzle_alloc()),
                            read_polled: read_polled1,
                            write_polled: write_polled1,
                        }),
                        rem_addr: Some(addr2.clone()),
                    }),
                    SocketType::Raw => return Outcome::Error(Errno::EOPNOTSUPP),
                },
            }),
            fizzle_alloc(),
        );

        let socket1_weak = Rc::downgrade(&socket1);

        let socket2 = Rc::new_in(
            RefCell::new(SocketInfo {
                fd_count: 1,
                socktype: self.socket_type,
                protocol: self.protocol,
                local_addr: LocalAddress::Assigned(addr2.addr().clone()),
                options: Default::default(),
                state: match self.socket_type {
                    SocketType::SeqPacket | SocketType::Stream => {
                        SocketState::Connected(ConnectedSocket {
                            backend: ConnectedBackend::Peered(RegularConnected {
                                peer: socket1_weak,
                                recv_buf: LinkedList::new_in(fizzle_alloc()),
                                read_idx: 0,
                                read_polled: read_polled2,
                                write_polled: write_polled2,
                            }),
                            rem_addr: addr1.clone(),
                            peer_closed: false,
                        })
                    }
                    SocketType::Datagram => SocketState::Connectionless(ConnectionlessSocket {
                        reuse_port,
                        backend: ConnectionlessBackend::Peered(RegularConnectionless {
                            recv_buf: LinkedList::new_in(fizzle_alloc()),
                            read_polled: read_polled2,
                            write_polled: write_polled2,
                        }),
                        rem_addr: Some(addr1.clone()),
                    }),
                    SocketType::Raw => unimplemented!(),
                },
            }),
            fizzle_alloc(),
        );

        let socket2_weak = Rc::downgrade(&socket2);

        match &mut socket2.borrow_mut().state {
            SocketState::Connected(connected_socket) => match &mut connected_socket.backend {
                ConnectedBackend::Peered(p) => p.peer = socket2_weak,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }

        state.local.fds.insert(
            fd1,
            DescriptorInfo {
                close_on_exec: self.cloexec,
                nonblocking: self.nonblocking,
                is_passthrough: false,
                is_random: false,
                resource: FdResource::Socket(socket1.clone()),
            },
        );

        state.local.fds.insert(
            fd2,
            DescriptorInfo {
                close_on_exec: self.cloexec,
                nonblocking: self.nonblocking,
                is_passthrough: false,
                is_random: false,
                resource: FdResource::Socket(socket2.clone()),
            },
        );

        let mut bound_sockets = VecDeque::new_in(fizzle_alloc());
        bound_sockets.push_back(socket1);

        state.global.socket_locations.insert(
            addr1,
            TransportLocationInfo {
                reuse_port,
                bound_sockets,
                pending: LinkedList::new_in(fizzle_alloc()),
            },
        );

        let mut bound_sockets = VecDeque::new_in(fizzle_alloc());
        bound_sockets.push_back(socket2);

        state.global.socket_locations.insert(
            addr2,
            TransportLocationInfo {
                reuse_port,
                bound_sockets,
                pending: LinkedList::new_in(fizzle_alloc()),
            },
        );

        Outcome::Success((fd1, fd2))
    }
}

pub struct SocketBindEvent<'a> {
    descriptor_id: Descriptor,
    addr_bytes: &'a [u8],
}

impl<'a> SocketBindEvent<'a> {
    pub fn new(descriptor_id: Descriptor, addr_bytes: &'a [u8]) -> Self {
        Self {
            descriptor_id,
            addr_bytes,
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

impl Event for SocketBindEvent<'_> {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        if fd_info.is_passthrough {
            let ret = unsafe {
                libc::bind(
                    self.descriptor_id.as_raw_fd(),
                    self.addr_bytes.as_ptr().cast::<libc::sockaddr>(),
                    self.addr_bytes.len() as u32,
                )
            };

            return if ret < 0 {
                Outcome::Error(Errno::get_errno())
            } else {
                Outcome::Success(())
            };
        }

        let FdResource::Socket(socket_info) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let Ok(mut sockaddr) = SockAddr::decode(self.addr_bytes) else {
            return Outcome::Error(Errno::EINVAL);
        };

        log::debug!(
            "binding socket {} to address {}...",
            self.descriptor_id.as_raw_fd(),
            sockaddr
        );

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

        let transport_addr = TransportAddress {
            sockaddr: sockaddr.clone(),
            protocol: socket_info.borrow().protocol,
        };

        match &socket_info.borrow().local_addr {
            LocalAddress::Ephemeral(_) => (),
            _ => {
                log::error!("attempt to re-bind socket that already had `bind()` called on it");
                return Outcome::Error(Errno::EINVAL);
            }
        }

        match &socket_info.borrow().state {
            SocketState::Server(_) | SocketState::Connecting(_) | SocketState::Connected(_) => {
                log::error!("socket in invalid state when binding (`listen()` or `connect()` already called)");
                return Outcome::Error(Errno::EINVAL);
            }
            _ => (),
        }

        let reuse_port = match &socket_info.borrow().state {
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
                    !(a.reuse_port && reuse_port) && !a.bound_sockets.is_empty()
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
                    location_info.bound_sockets.push_back(socket_info.clone());
                    location_info.reuse_port = reuse_port;
                } else {
                    log::warn!("socket attempted to bind to bound address {}", o.key());
                    return Outcome::Error(Errno::EADDRINUSE);
                }
            }
            Entry::Vacant(v) => {
                let mut bound_sockets = VecDeque::new_in(fizzle_alloc());
                bound_sockets.push_back(socket_info.clone());

                v.insert(TransportLocationInfo {
                    reuse_port,
                    bound_sockets,
                    pending: LinkedList::new_in(fizzle_alloc()),
                });
            }
        }

        // Swap the address out properly
        let _ = mem::replace(
            &mut socket_info.borrow_mut().local_addr,
            LocalAddress::Assigned(sockaddr),
        );

        Outcome::Success(())
    }
}

pub struct SocketListenEvent {
    descriptor_id: Descriptor,
    _backlog: libc::c_int, // Not actually used right now
}

impl SocketListenEvent {
    pub fn new(descriptor_id: Descriptor, backlog: libc::c_int) -> Self {
        Self {
            descriptor_id,
            _backlog: backlog,
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

        let FdResource::Socket(mut socket_info) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let addr = get_or_assign_local(&mut socket_info, state);

        let mut sock_info_mut = socket_info.borrow_mut();
        let SocketState::Unassociated(_) = &sock_info_mut.state else {
            log::error!(
                "calling listen() on a connected or listening socket unsupported by Fizzle"
            );
            return Outcome::Error(Errno::EINVAL);
        };

        // Allocate server context and set up polling
        let ready_to_connect = Rc::new_in(
            RefCell::new(PolledInfo {
                pollers: Vec::new_in(fizzle_alloc()),
                event_raised: false,
            }),
            fizzle_alloc(),
        );

        let connecting = mem::replace(
            &mut state
                .global
                .socket_locations
                .get_mut(&addr)
                .unwrap()
                .pending,
            LinkedList::new_in(fizzle_alloc()),
        );

        if !connecting.is_empty() {
            log::debug!("listen() socket had pending connections--raising polled");
            state.raise_polled(&ready_to_connect);
        }

        // In the case of an ephemeral address, the concrete address now needs to be assigned to the socket
        sock_info_mut.local_addr = LocalAddress::Assigned(addr.addr().clone());

        sock_info_mut.state = SocketState::Server(ServerSocket {
            backend: ServerBackend::Peered(()),
            connecting,
            ready_to_connect,
        });

        Outcome::Success(())
    }
}

pub enum SocketConnectState {
    Start,
    Finish(GlobalRc<PollerInfo>),
}

pub struct SocketConnectEvent {
    descriptor_id: Descriptor,
    dst_addr: SockAddr,
    state: SocketConnectState,
}

impl SocketConnectEvent {
    pub fn new(descriptor_id: Descriptor, dst_addr: SockAddr) -> Self {
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

                let FdResource::Socket(mut socket_info) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::ENOTSOCK);
                };

                let _addr = get_or_assign_local(&mut socket_info, state);

                let mut borrowed_socket_info = socket_info.borrow_mut();
                let protocol = borrowed_socket_info.protocol;

                match &mut borrowed_socket_info.state {
                    SocketState::Unassociated(_) => {
                        let dst_addr = TransportAddress {
                            sockaddr: self.dst_addr.clone(),
                            protocol,
                        };

                        let server_socket_info: GlobalRc<SocketInfo>;

                        // TODO: distribute evenly across multiple listening sockets
                        if let Some(socket_info) = state
                            .global
                            .socket_locations
                            .get(&dst_addr)
                            .and_then(|l| l.bound_sockets.front())
                        {
                            // The exact address is bound
                            server_socket_info = socket_info.clone();
                        } else if let Some(socket_info) = dst_addr
                            .wildcard()
                            .and_then(|t| state.global.socket_locations.get(&t))
                            .and_then(|l| l.bound_sockets.front())
                        {
                            // The wildcard address is bound
                            server_socket_info = socket_info.clone();
                        } else {
                            // No socket is bound to the given address...
                            log::warn!(
                                "connect() on address {} failed--no server listening",
                                &dst_addr
                            );
                            return Outcome::Error(Errno::ECONNREFUSED);
                        }

                        let mut server_sock_mut = server_socket_info.borrow_mut();
                        let SocketState::Server(server_info) = &mut server_sock_mut.state else {
                            log::error!("inconsistent Fizzle state--socket bound to connect() address {} not in listening state", &dst_addr);
                            return Outcome::Error(Errno::ECONNREFUSED);
                        };

                        let server_backend = server_info.backend.clone();
                        let connecting = &mut server_info.connecting;

                        let connected_backend = match server_backend {
                            ServerBackend::Passthrough => unreachable!(),
                            ServerBackend::Peered(()) => {
                                if connecting.len() >= FIZZLE_SOMAXCONN {
                                    return Outcome::Error(Errno::ECONNABORTED);
                                }
                                connecting.push_back(socket_info.clone());

                                let server_poll = server_info.ready_to_connect.clone();
                                state.raise_polled(&server_poll);

                                let client_poll = Rc::new_in(
                                    RefCell::new(PolledInfo {
                                        pollers: Vec::new_in(fizzle_alloc()),
                                        event_raised: false,
                                    }),
                                    fizzle_alloc(),
                                );

                                borrowed_socket_info.state = SocketState::Connecting(ConnectingSocket {
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
                                        return Outcome::Yield(YieldUntil::None);
                                    }
                                }
                            }
                            ServerBackend::Plugin(plugin_info) => {
                                // Create new plugin
                                let endpoint = plugin_info.borrow().endpoint.clone();
                                let module = plugin_info.borrow().module.clone();
                                let connect_plugin_id = state.global.add_plugin(endpoint, module);
                                ConnectedBackend::Plugin(connect_plugin_id)
                            }
                            ServerBackend::Sink => ConnectedBackend::Sink,
                            ServerBackend::Fuzz(_) => {
                                ConnectedBackend::Fuzz(state.global.add_fuzz_endpoint())
                            }
                            ServerBackend::NullSink => ConnectedBackend::NullSink,
                            ServerBackend::Feedback(()) => {
                                ConnectedBackend::Feedback(StandardFeedback {
                                    buf: LinkedList::new_in(fizzle_alloc()),
                                    read_polled: Rc::new_in(
                                        RefCell::new(PolledInfo {
                                            pollers: Vec::new_in(fizzle_alloc()),
                                            event_raised: false,
                                        }),
                                        fizzle_alloc(),
                                    ),
                                    read_idx: 0,
                                    write_polled: Rc::new_in(
                                        RefCell::new(PolledInfo {
                                            pollers: Vec::new_in(fizzle_alloc()),
                                            event_raised: true,
                                        }),
                                        fizzle_alloc(),
                                    ),
                                })
                            }
                        };

                        // Fix double mutable borrow
                        drop(server_sock_mut);

                        server_socket_info.borrow_mut().state =
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
                                return Outcome::Yield(YieldUntil::Immediate);
                            } else {
                                let poller_id = state.new_poller();
                                state.register_poller(poller_id.clone(), client_poll);
                                self.state = SocketConnectState::Finish(poller_id);

                                // TODO: for `SO_SNDTIMEO`, this should be Some()
                                return Outcome::Yield(YieldUntil::None);
                            }
                        }
                    }
                    SocketState::Connected(_) => Outcome::Error(Errno::EISCONN),
                    SocketState::Connectionless(conn) => {
                        let dst_addr = TransportAddress {
                            sockaddr: self.dst_addr.clone(),
                            protocol,
                        };

                        conn.rem_addr = Some(dst_addr);
                        Outcome::Success(())
                    }
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
    Blocked(GlobalRc<PollerInfo>),
    Finish(GlobalRc<SocketInfo>, TransportAddress),
}

pub struct SocketAcceptEvent {
    descriptor_id: Descriptor,
    nonblock: bool,
    cloexec: bool,
    state: SocketAcceptState,
}

impl SocketAcceptEvent {
    pub fn new(descriptor_id: Descriptor, nonblock: bool, cloexec: bool) -> Self {
        Self {
            descriptor_id,
            nonblock,
            cloexec,
            state: SocketAcceptState::Start,
        }
    }
}

impl Event for SocketAcceptEvent {
    type Success = (Descriptor, SockAddr);
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &self.state {
            SocketAcceptState::Start => {
                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    return Outcome::Error(Errno::EBADF);
                };

                self.nonblock |= fd_info.nonblocking;
                // TODO: does socket inherit CLOEXEC mode too?

                let nonblocking = fd_info.nonblocking;

                let FdResource::Socket(server_socket_info) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::ENOTSOCK);
                };

                let mut server_sock_mut = server_socket_info.borrow_mut();
                let protocol = server_sock_mut.protocol;
                let LocalAddress::Assigned(sockaddr) = server_sock_mut.local_addr.clone() else {
                    unreachable!()
                };

                let SocketState::Server(server_info) = &mut server_sock_mut.state else {
                    log::error!("accept() called on non-listening socket");
                    return Outcome::Error(Errno::EINVAL);
                };

                let server_poll = server_info.ready_to_connect.clone();
                let has_connecting = !server_info.connecting.is_empty();
                let server_address = TransportAddress { sockaddr, protocol };

                let bound_info = state
                    .global
                    .socket_locations
                    .get_mut(&server_address)
                    .unwrap();
                if let Some(mut client) = bound_info.pending.pop_front() {
                    if bound_info.pending.is_empty() && !has_connecting {
                        state.lower_polled(&server_poll);
                    }

                    let _addr = get_or_assign_local(&mut client, state);

                    let client_ref = client.borrow();
                    assert!(matches!(
                        &client_ref.state,
                        SocketState::PendingConnection(_)
                    ));

                    self.state = SocketAcceptState::Finish(client.clone(), server_address);
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    let SocketState::Server(server_info) = &mut server_sock_mut.state else {
                        unreachable!()
                    };

                    if let Some(mut connecting_info) = server_info.connecting.pop_front() {
                        let _addr = get_or_assign_local(&mut connecting_info, state);

                        if server_info.connecting.is_empty() {
                            state.lower_polled(&server_poll);
                        }

                        let mut connecting_info_mut = connecting_info.borrow_mut();

                        let SocketState::Connecting(connecting_socket_info) =
                            &mut connecting_info_mut.state
                        else {
                            unreachable!()
                        };

                        let connect_polled = connecting_socket_info.connect_polled.clone();
                        drop(connecting_info_mut);
                        state.raise_polled(&connect_polled);

                        self.state =
                            SocketAcceptState::Finish(connecting_info.clone(), server_address);
                        Outcome::Yield(YieldUntil::Immediate)
                    } else if nonblocking {
                        Outcome::Error(Errno::EAGAIN)
                    } else {
                        if state.polled_is_ready(&server_poll) {
                            log::warn!("accept() poller was unexpectedly raised");
                            state.lower_polled(&server_poll);
                            // panic!("`accept()` poller in unexpected state");
                        }

                        let poller_id = state.new_poller();
                        state.register_poller(poller_id.clone(), server_poll);

                        self.state = SocketAcceptState::Blocked(poller_id);
                        // TODO: for `SO_SNDTIMEO`, this should be Some()
                        Outcome::Yield(YieldUntil::None)
                    }
                }
            }
            SocketAcceptState::Blocked(poller_id) => {
                state.delete_poller(poller_id.clone());

                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    log::error!("socket unexpectedly closed during `accept()`");
                    return Outcome::Error(Errno::EBADF);
                };

                let FdResource::Socket(mut server_socket_info) = fd_info.resource.clone() else {
                    return Outcome::Error(Errno::ENOTSOCK);
                };

                let server_address = get_or_assign_local(&mut server_socket_info, state);

                let SocketState::Server(server_info) = &mut server_socket_info.borrow_mut().state
                else {
                    log::error!("socket state unexpectedly changed during `accept()`");
                    return Outcome::Error(Errno::EINVAL);
                };

                let server_polled = server_info.ready_to_connect.clone();
                let more_connecting = !server_info.connecting.is_empty();

                let mut connecting_info = server_info.connecting.pop_front().unwrap();
                get_or_assign_local(&mut connecting_info, state);

                match &connecting_info.borrow().state {
                    SocketState::Connecting(_) | SocketState::PendingConnection(_) => (),
                    _ => unreachable!(),
                }

                if !more_connecting {
                    state.lower_polled(&server_polled);
                }

                self.state = SocketAcceptState::Finish(connecting_info.clone(), server_address);
                Outcome::Yield(YieldUntil::Immediate)
            }
            SocketAcceptState::Finish(connecting_info, server_address) => {
                let mut connecting_info = connecting_info.clone();
                // `connecting` corresponds to the client, while `accepting` corresponds to the new
                // socket created by the server to accept the connection.

                let connecting_address = get_or_assign_local(&mut connecting_info, state);

                // TODO: not needed, I guess?
                /*
                let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
                    log::error!("socket unexpectedly closed during `accept()`");
                    return Outcome::Error(Errno::EBADF);
                };
                */

                let close_on_exec = self.cloexec;

                let socktype = connecting_info.borrow().socktype;
                let protocol = connecting_info.borrow().protocol;
                let mut connecting_info_mut = connecting_info.borrow_mut();

                let connecting_backend = match &mut connecting_info_mut.state {
                    SocketState::Connecting(connecting_socket) => {
                        state.raise_polled(&connecting_socket.connect_polled);
                        connecting_socket.backend.clone()
                    }
                    SocketState::PendingConnection(pending_socket) => {
                        pending_socket.backend.clone()
                    }
                    _ => unreachable!(),
                };
                drop(connecting_info_mut);

                let accepting_backend = match connecting_backend {
                    ConnectingBackend::Passthrough => unreachable!(),
                    ConnectingBackend::Peered(()) => ConnectedBackend::Peered(RegularConnected {
                        peer: Rc::downgrade(&connecting_info),
                        recv_buf: LinkedList::new_in(fizzle_alloc()),
                        read_polled: Rc::new_in(
                            RefCell::new(PolledInfo {
                                pollers: Vec::new_in(fizzle_alloc()),
                                event_raised: false,
                            }),
                            fizzle_alloc(),
                        ),
                        read_idx: 0,
                        write_polled: Rc::new_in(
                            RefCell::new(PolledInfo {
                                pollers: Vec::new_in(fizzle_alloc()),
                                event_raised: true,
                            }),
                            fizzle_alloc(),
                        ),
                    }),
                    ConnectingBackend::Feedback(()) => {
                        ConnectedBackend::Feedback(StandardFeedback {
                            buf: LinkedList::new_in(fizzle_alloc()),
                            read_polled: Rc::new_in(
                                RefCell::new(PolledInfo {
                                    pollers: Vec::new_in(fizzle_alloc()),
                                    event_raised: false,
                                }),
                                fizzle_alloc(),
                            ),
                            read_idx: 0,
                            write_polled: Rc::new_in(
                                RefCell::new(PolledInfo {
                                    pollers: Vec::new_in(fizzle_alloc()),
                                    event_raised: true,
                                }),
                                fizzle_alloc(),
                            ),
                        })
                    }
                    ConnectingBackend::Plugin(plugin_info) => ConnectedBackend::Plugin(plugin_info),
                    ConnectingBackend::Sink => ConnectedBackend::Sink,
                    ConnectingBackend::NullSink => ConnectedBackend::NullSink,
                    ConnectingBackend::Fuzz(endpoint) => ConnectedBackend::Fuzz(endpoint),
                };

                let accepting_info = if let ConnectedBackend::Peered(_) = accepting_backend {
                    let accepting_info = Rc::new_in(
                        RefCell::new(SocketInfo {
                            fd_count: 1,
                            socktype,
                            protocol,
                            local_addr: LocalAddress::Assigned(server_address.sockaddr.clone()),
                            options: Default::default(),
                            state: SocketState::Connected(ConnectedSocket {
                                rem_addr: connecting_address.clone(),
                                backend: accepting_backend,
                                peer_closed: false,
                            }),
                        }),
                        fizzle_alloc(),
                    );

                    let connected_backend = ConnectedBackend::Peered(RegularConnected {
                        peer: Rc::downgrade(&accepting_info),
                        recv_buf: LinkedList::new_in(fizzle_alloc()),
                        read_polled: Rc::new_in(
                            RefCell::new(PolledInfo {
                                pollers: Vec::new_in(fizzle_alloc()),
                                event_raised: false,
                            }),
                            fizzle_alloc(),
                        ),
                        read_idx: 0,
                        write_polled: Rc::new_in(
                            RefCell::new(PolledInfo {
                                pollers: Vec::new_in(fizzle_alloc()),
                                event_raised: true,
                            }),
                            fizzle_alloc(),
                        ),
                    });

                    connecting_info.borrow_mut().state = SocketState::Connected(ConnectedSocket {
                        rem_addr: server_address.clone(),
                        backend: connected_backend,
                        peer_closed: false,
                    });

                    accepting_info
                } else {
                    // The connecting socket was emulated in some way (`fuzz`, `sink` or the like).
                    // Convert the connecting socket into the accepted socket--we don't need two peered sockets.

                    connecting_info.borrow_mut().local_addr =
                        LocalAddress::Assigned(server_address.sockaddr.clone());
                    connecting_info.borrow_mut().state = SocketState::Connected(ConnectedSocket {
                        rem_addr: connecting_address.clone(),
                        backend: accepting_backend,
                        peer_closed: false,
                    });

                    connecting_info.clone()
                };

                log::debug!("accepted new socket with nonblocking: {}", self.nonblock);
                let new_fd = Descriptor::from_raw_fd(crate::create_descriptor());
                // The two sockets are now joined--add a file descriptor to the accepted socket
                state.local.fds.insert(
                    new_fd,
                    DescriptorInfo {
                        close_on_exec,
                        is_passthrough: false,
                        nonblocking: self.nonblock,
                        is_random: false,
                        resource: FdResource::Socket(accepting_info),
                    },
                );

                Outcome::Success((new_fd, connecting_address.sockaddr.clone()))
            }
        }
    }
}

pub struct SocketGetNameEvent<'a> {
    descriptor_id: Descriptor,
    addr_bytes: &'a mut [MaybeUninit<u8>],
}

impl<'a> SocketGetNameEvent<'a> {
    pub fn new(descriptor_id: Descriptor, addr_bytes: &'a mut [MaybeUninit<u8>]) -> Self {
        Self {
            descriptor_id,
            addr_bytes,
        }
    }
}

impl Event for SocketGetNameEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        if fd_info.is_passthrough {
            let mut addrlen = self.addr_bytes.len() as u32;
            let ret = unsafe {
                libc::getsockname(
                    self.descriptor_id.as_raw_fd(),
                    self.addr_bytes.as_mut_ptr().cast::<libc::sockaddr>(),
                    &raw mut addrlen,
                )
            };

            return if ret < 0 {
                Outcome::Error(Errno::get_errno())
            } else {
                Outcome::Success(addrlen as usize)
            };
        }

        let FdResource::Socket(socket_info) = &fd_info.resource else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        match &socket_info.borrow().local_addr {
            LocalAddress::Ephemeral(address_family) => {
                self.addr_bytes.fill(MaybeUninit::new(0));
                let family_bytes = address_family
                    .raw()
                    .to_be_bytes()
                    .map(|i| MaybeUninit::new(i));

                let family_bytelen = cmp::min(family_bytes.len(), family_bytes.len());
                self.addr_bytes[..family_bytelen].copy_from_slice(&family_bytes);
                Outcome::Success(family_bytelen)
            }
            LocalAddress::Assigned(sock_addr) => {
                let addrlen = sock_addr.encode(self.addr_bytes);
                Outcome::Success(addrlen)
            }
        }
    }
}

pub struct SocketGetPeerNameEvent {
    descriptor_id: Descriptor,
}

impl SocketGetPeerNameEvent {
    pub fn new(descriptor_id: Descriptor) -> Self {
        Self { descriptor_id }
    }
}

impl Event for SocketGetPeerNameEvent {
    type Success = SockAddr;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.descriptor_id) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Socket(socket_info) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let socket_ref = socket_info.borrow();
        let addr_opt = match &socket_ref.state {
            SocketState::Connectionless(connectionless) => {
                connectionless.rem_addr.as_ref().map(|a| a.addr())
            }
            SocketState::Unassociated(_) => None,
            SocketState::Server(_) => None,
            SocketState::PendingConnection(_) => None,
            SocketState::Connecting(_) => None,
            SocketState::Connected(connected) => Some(connected.rem_addr.addr()),
        };

        match addr_opt {
            Some(addr) => Outcome::Success(addr.clone()),
            None => Outcome::Error(Errno::ENOTCONN),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SctpRtoInfo {
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
    pub sasoc_number_peer_destinations: u16,
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
    SocketPeerCred(libc::ucred),
    SocketIsListening(bool),
    SocketDontRoute(bool),
    SocketDomain(AddressFamily),
    SocketType(SocketType),
    SocketError(libc::c_int),
    SocketKeepalive(bool),
    SocketLinger(Option<u32>),
    SocketOobInline(bool),
    SocketZeroCopy(bool),
    SocketPriority(u32),
    SocketProtocol(TransportProtocol),
    SocketRecvBuffer(u32),
    SocketSendBuffer(u32),
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
            Self::SocketPeerCred(ucred) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &linger to &[u8]
                let ucred_bytes: &[u8] = unsafe { slice::from_ref(&ucred).align_to().1 };
                assert!(
                    ucred_bytes.len() == mem::size_of_val(&ucred),
                    "align_to() failed to convert `libc::ucred` to bytes"
                );
                
                for (dst, src) in out.iter_mut().zip(ucred_bytes) {
                    dst.write(*src);
                }

               ucred_bytes.len()
            }
            Self::IpOptions(v) | Self::SctpGetLocalAddrs(v) => {
                for (dst, src) in out.iter_mut().zip(v) {
                    dst.write(*src);
                }

                v.len()
            }
            Self::TcpUserTimeout(d) => {
                let millis: libc::c_uint = d.as_millis().try_into().unwrap();
                let millis_bytes = millis.to_ne_bytes();

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

                let flag_bytes = flag.to_ne_bytes();

                for (dst, src) in out.iter_mut().zip(flag_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&flag)
            }
            Self::TcpMss(u)
            | Self::SocketPriority(u)
            | Self::SocketRecvBuffer(u)
            | Self::SocketSendBuffer(u)
            | Self::SocketSendLowWatermark(u)
            | Self::SocketRecvLowWatermark(u)
            | Self::SctpMaxSegment(u) => {
                let u_bytes = u.to_ne_bytes();

                for (dst, src) in out.iter_mut().zip(u_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&u)
            }
            Self::SocketDomain(f) => {
                let domain_bytes = f.raw().to_ne_bytes();

                for (dst, src) in out.iter_mut().zip(domain_bytes) {
                    dst.write(src);
                }

                mem::size_of_val(&f.raw())
            }
            Self::SocketType(t) => {
                let type_bytes = match t {
                    SocketType::Stream => libc::SOCK_STREAM,
                    SocketType::Datagram => libc::SOCK_DGRAM,
                    SocketType::SeqPacket => libc::SOCK_SEQPACKET,
                    SocketType::Raw => libc::SOCK_RAW,
                }
                .to_ne_bytes();

                for (dst, src) in out.iter_mut().zip(type_bytes) {
                    dst.write(src);
                }

                mem::size_of::<libc::c_int>()
            }
            Self::SocketError(error) => {
                let error_bytes = error.to_ne_bytes();

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

                let p_bytes = p_int.to_ne_bytes();

                for (dst, src) in out.iter_mut().zip(p_bytes) {
                    dst.write(src);
                }

                p_bytes.len()
            }
            Self::SctpRtoInfo(rto_info) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &rto_info to &[u8]
                let rto_info_bytes: &[u8] = unsafe {
                    slice::from_raw_parts(
                        (rto_info as *const SctpRtoInfo).cast::<u8>(),
                        mem::size_of::<SctpRtoInfo>(),
                    )
                };
                assert!(
                    rto_info_bytes.len() == mem::size_of_val(rto_info),
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
                let addr_param_bytes: &[u8] = unsafe {
                    slice::from_raw_parts(
                        (addr_params as *const SctpPeerAddrParams).cast::<u8>(),
                        mem::size_of_val(addr_params),
                    )
                };

                for (dst, src) in out.iter_mut().zip(addr_param_bytes) {
                    dst.write(*src);
                }

                addr_param_bytes.len()
            }
            Self::SctpEvents(events_subscribe) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &SctpPeerAddrParams to &[u8]
                let events_subscribe_bytes: &[u8] = unsafe {
                    slice::from_raw_parts(
                        (events_subscribe as *const SctpEventSubscribe).cast::<u8>(),
                        mem::size_of_val(events_subscribe),
                    )
                };

                for (dst, src) in out.iter_mut().zip(events_subscribe_bytes) {
                    dst.write(*src);
                }

                events_subscribe_bytes.len()
            }
            Self::SctpAssocInfo(assoc_params) => {
                // SAFETY: u8 never should have alignment issues, so this should turn &SctpPeerAddrParams to &[u8]
                let assoc_params_bytes: &[u8] = unsafe {
                    slice::from_raw_parts(
                        (assoc_params as *const SctpAssocParams).cast::<u8>(),
                        mem::size_of_val(assoc_params),
                    )
                };

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
    descriptor_id: Descriptor,
    optlevel: OptLevel,
    optname: libc::c_int,
    input: OptInput,
}

impl SocketGetOptionEvent {
    pub fn new(
        descriptor_id: Descriptor,
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

        let FdResource::Socket(mut socket_info) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        let family = socket_info.borrow().local_addr.family();
        let protocol = socket_info.borrow().protocol;

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
                if let SocketState::Server(_) = &socket_info.borrow().state {
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
                socket_info.borrow().local_addr.family(),
            )),
            (OptLevel::Socket, libc::SO_ERROR) => {
                // TODO: pass errors raised during polling here
                Outcome::Success(SocketOption::SocketError(0))
            }
            (OptLevel::Socket, libc::SO_PEERCRED) => {
                Outcome::Success(SocketOption::SocketPeerCred(libc::ucred {
                    pid: 0,
                    gid: 0,
                    uid: 0,
                }))
            }
            (OptLevel::Socket, libc::SO_KEEPALIVE) => {
                // TODO: implement assignment of this flag
                Outcome::Success(SocketOption::SocketKeepalive(false))
            }
            (OptLevel::Socket, libc::SO_SNDBUF) => {
                // TODO: make dynamic based on buffer set by user
                Outcome::Success(SocketOption::SocketSendBuffer(65536))
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
                Outcome::Success(SocketOption::SocketProtocol(socket_info.borrow().protocol))
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
                let borrowed_socket_info = socket_info.borrow();
                Outcome::Success(SocketOption::SocketReusePort(
                    match &borrowed_socket_info.state {
                        SocketState::Connectionless(connectionless_socket) => {
                            connectionless_socket.reuse_port
                        }
                        SocketState::Unassociated(unassociated_socket) => {
                            unassociated_socket.reuse_port
                        }
                        _ => {
                            drop(borrowed_socket_info);
                            let transport_addr = get_or_assign_local(&mut socket_info, state);
                            state
                                .global
                                .socket_locations
                                .get(&transport_addr)
                                .unwrap()
                                .reuse_port
                        }
                    },
                ))
            }
            (OptLevel::Socket, libc::SO_TYPE) => {
                Outcome::Success(SocketOption::SocketType(socket_info.borrow().socktype))
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
            // TODO: handle mapped V4-V6 addresses
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
            (OptLevel::Sctp, SCTP_SOCKOPT_CONNECTX3) => {
                todo!() // TODO: continue on from here
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
                    sasoc_number_peer_destinations: 1,
                    sasoc_peer_rwnd: 1,
                    sasoc_local_rwnd: 1,
                    sasoc_cookie_life: 60000,
                }))
            }
            (OptLevel::Sctp, SCTP_SOCKOPT_CONNECTX | SCTP_SOCKOPT_CONNECTX3) => {
                // TODO: implement
                Outcome::Error(Errno::EINVAL)
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
                let sock_ref = socket_info.borrow();
                if sock_ref.protocol != TransportProtocol::Tcp {
                    log::warn!("getsockopt(SOL_TCP, TCP_USER_TIMEOUT) called on non-TCP socket");
                    return Outcome::Error(Errno::ENOPROTOOPT);
                }

                Outcome::Success(SocketOption::TcpUserTimeout(
                    sock_ref.options.tcp_user_timeout.unwrap_or(Duration::ZERO),
                ))
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
    descriptor_id: Descriptor,
    option: SocketOption,
}

impl SocketSetOptionEvent {
    pub fn new(descriptor_id: Descriptor, option: SocketOption) -> Self {
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

        let FdResource::Socket(mut socket_info) = fd_info.resource.clone() else {
            return Outcome::Error(Errno::ENOTSOCK);
        };

        match &self.option {
            SocketOption::SocketSendBuffer(_)
            | SocketOption::SocketIsListening(_)
            | SocketOption::SocketDomain(_)
            | SocketOption::SocketType(_)
            | SocketOption::SocketError(_)
            | SocketOption::SocketProtocol(_)
            | SocketOption::SocketPeerCred(_)
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
                let mut borrowed_socket = socket_info.borrow_mut();
                match &mut borrowed_socket.state {
                    SocketState::Connectionless(connectionless_socket) => {
                        connectionless_socket.reuse_port = *reuse
                    }
                    SocketState::Unassociated(unassociated_socket) => {
                        unassociated_socket.reuse_port = *reuse
                    }
                    _ => {
                        drop(borrowed_socket);
                        let transport_addr = get_or_assign_local(&mut socket_info, state);
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
            SocketOption::TcpUserTimeout(duration) => {
                let mut sock_mut = socket_info.borrow_mut();
                if sock_mut.protocol != TransportProtocol::Tcp {
                    log::warn!("setsockopt(SOL_TCP, TCP_USER_TIMEOUT) called on non-TCP socket");
                    return Outcome::Error(Errno::ENOPROTOOPT);
                }

                sock_mut.options.tcp_user_timeout = if duration == &Duration::ZERO {
                    None
                } else {
                    Some(*duration)
                };

                Outcome::Success(())
            }
            // TODO: implement
            SocketOption::TcpNoDelay(_) => Outcome::Success(()),
            // TODO: implement
            SocketOption::TcpMss(_) => Outcome::Success(()),
        }
    }
}

pub enum SocketReadState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

#[repr(C)]
pub struct SctpShutdownEvent {
    pub sse_type: u16,
    pub sse_flags: u16,
    pub sse_length: u32,
    pub sse_assoc_id: libc::sctp_assoc_t,
}

pub const SCTP_SHUTDOWN_EVENT: u16 = (1 << 15) + 5;

pub struct SocketReadEvent<'a> {
    socket: GlobalRc<SocketInfo>,
    nonblocking: bool,
    data: ReadData<'a>,
    state: SocketReadState,
}

impl<'a> SocketReadEvent<'a> {
    #[inline]
    pub fn new(socket: GlobalRc<SocketInfo>, nonblocking: bool, data: ReadData<'a>) -> Self {
        Self {
            socket,
            nonblocking,
            data,
            state: SocketReadState::Start,
        }
    }
}

impl Event for SocketReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let socket_info = self.socket.clone();
        let mut borrowed_socket_info = socket_info.borrow_mut();

        match (&self.state, &mut borrowed_socket_info.state) {
            (SocketReadState::Start, _) => {
                let read_polled = match &borrowed_socket_info.state {
                    SocketState::Connectionless(c) => c.read_polled(),
                    SocketState::Connected(c) => {
                        // If the socket has *just* connected, it may have a raised read poll (for the sake of alerting `connect()`).
                        let has_data = match &c.backend {
                            ConnectedBackend::Passthrough => true,
                            ConnectedBackend::Peered(p) => !p.recv_buf.is_empty(),
                            ConnectedBackend::Feedback(f) => !f.buf.is_empty(),
                            ConnectedBackend::Plugin(p) => !p.borrow().read_buf.is_empty(),
                            ConnectedBackend::Sink => false,
                            ConnectedBackend::NullSink => true,
                            ConnectedBackend::Fuzz(rc) => rc.borrow().read_idx < state.global.fuzz_input.len(),
                        };

                        c.read_polled().and_then(|p| {
                            if !has_data {
                                state.lower_polled(&p);
                            }

                            Some(p)
                        })
                    },
                    _ => return Outcome::Error(Errno::ENOTCONN),
                };

                if let Some(read_polled) = read_polled.as_ref() {
                    if state.polled_is_ready(&read_polled) {
                        self.state = SocketReadState::Finish(None);
                        Outcome::Yield(YieldUntil::Immediate)
                    } else if self.nonblocking {
                        Outcome::Error(Errno::EAGAIN)
                    } else {
                        let poller_id = state.new_poller();
                        state.register_poller(poller_id.clone(), read_polled.clone());

                        self.state = SocketReadState::Finish(Some(poller_id));
                        Outcome::Yield(YieldUntil::None)
                    }
                } else {
                    self.state = SocketReadState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                }
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Passthrough,
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }
                unimplemented!()
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Peered(regular),
                    ..
                }),
            ) => {
                let read_polled = &regular.read_polled;

                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    ReadData::BasicSlice(s) => {
                        let message = regular.recv_buf.pop_front().unwrap();

                        let read = cmp::min(s.len(), message.data.len());
                        s.copy_from_slice(&message.data[..read]);

                        if regular.recv_buf.is_empty() {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(read)
                    }
                    ReadData::Iovec(data) => {
                        let message = regular.recv_buf.pop_front().unwrap();

                        let mut idx = 0;
                        for s in data.iter_mut() {
                            let read = cmp::min(s.len(), message.data.len() - idx);
                            s[..read].copy_from_slice(&message.data[idx..idx + read]);
                            idx += read;
                        }

                        if regular.recv_buf.is_empty() {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(idx)
                    }
                    ReadData::File(_data) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        // TODO: blocking incorrectly handled here (see the MSG_WAITFORONE flag in `man 2 recvmmsg`)

                        let mut msg_count = 0;
                        for out_msg in out_msgs.iter_mut() {
                            let Some(msg) = regular.recv_buf.pop_front() else {
                                assert!(msg_count > 0);
                                state.lower_polled(&read_polled);
                                return Outcome::Success(msg_count);
                            };

                            *out_msg.msg_flags = SocketMsgFlags::EOR;

                            *out_msg.addrlen = msg.source.encode(&mut out_msg.addr_bytes) as u32;
                            for (out_byte, byte) in
                                out_msg.control_info.iter_mut().zip(msg.ancillary.iter())
                            {
                                out_byte.write(*byte);
                            }

                            *out_msg.control_len =
                                cmp::min(*out_msg.control_len, msg.ancillary.len());
                            if *out_msg.control_len < msg.ancillary.len() {
                                *out_msg.msg_flags |= SocketMsgFlags::CTRUNC;
                            }

                            let mut total_read = 0;
                            for s in out_msg.buf.iter_mut() {
                                let read = cmp::min(msg.data.len() - total_read, s.len());
                                s[..read].copy_from_slice(&msg.data[total_read..total_read + read]);
                                total_read += read;
                            }

                            if total_read < msg.data.len() {
                                *out_msg.msg_flags |= SocketMsgFlags::TRUNC;
                            }

                            *out_msg.buflen = total_read as u32;

                            msg_count += 1;
                        }

                        if regular.recv_buf.is_empty() {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(msg_count)
                    }
                }
            }
            (
                SocketReadState::Finish(_poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Feedback(_feedback),
                    ..
                }),
            ) => {
                unreachable!()
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Plugin(_plugin_endpoint_id),
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                todo!("stateless socket plugins (e.g. UDP) not implemented")
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Sink,
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                Outcome::Success(0)
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::NullSink,
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    ReadData::BasicSlice(out_slice) => {
                        out_slice.fill(0);
                        Outcome::Success(out_slice.len())
                    }
                    ReadData::Iovec(out_slices) => {
                        let mut total_read = 0;
                        for s in out_slices.iter_mut() {
                            for b in s.iter_mut() {
                                *b = 0;
                            }
                            total_read += s.len();
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::File(_file_read_data) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        for msg in out_msgs.iter_mut() {
                            *msg.addrlen = 0;
                            *msg.control_len = 0;
                            for s in msg.buf.iter_mut() {
                                for b in s.iter_mut() {
                                    *b = 0;
                                }
                            }
                        }

                        Outcome::Success(out_msgs.len())
                    }
                }
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Fuzz(fuzz_endpoint),
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    ReadData::BasicSlice(s) => {
                        let read = cmp::min(
                            s.len(),
                            state.global.fuzz_input.len() - fuzz_endpoint.borrow().read_idx,
                        );
                        s.copy_from_slice(
                            &state.global.fuzz_input[fuzz_endpoint.borrow().read_idx
                                ..fuzz_endpoint.borrow().read_idx + read],
                        );
                        fuzz_endpoint.borrow_mut().read_idx += read;

                        Outcome::Success(read)
                    }
                    ReadData::Iovec(out_slices) => {
                        let mut total_read = 0;
                        for s in out_slices.iter_mut() {
                            let read = cmp::min(
                                s.len(),
                                state.global.fuzz_input.len() - fuzz_endpoint.borrow().read_idx,
                            );
                            s.copy_from_slice(
                                &state.global.fuzz_input[fuzz_endpoint.borrow().read_idx
                                    ..fuzz_endpoint.borrow().read_idx + read],
                            );
                            fuzz_endpoint.borrow_mut().read_idx += read;
                            total_read += read;
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::File(_) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        let mut total_read = 0;
                        for out_msg in out_msgs.iter_mut() {
                            for s in out_msg.buf.iter_mut() {
                                let read = cmp::min(
                                    s.len(),
                                    state.global.fuzz_input.len() - fuzz_endpoint.borrow().read_idx,
                                );
                                s.copy_from_slice(
                                    &state.global.fuzz_input[fuzz_endpoint.borrow().read_idx
                                        ..fuzz_endpoint.borrow().read_idx + read],
                                );
                                fuzz_endpoint.borrow_mut().read_idx += read;
                                total_read += read;
                            }
                        }

                        Outcome::Success(total_read)
                    }
                }
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::Passthrough,
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                unimplemented!()
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::Peered(regular),
                    rem_addr,
                    peer_closed,
                }),
            ) => {
                let read_polled = &regular.read_polled;

                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                let sockaddr = rem_addr.addr().clone();

                match &mut self.data {
                    ReadData::BasicSlice(data) => {
                        let Some(buf) = regular.recv_buf.pop_front() else {
                            if *peer_closed {
                                return Outcome::Success(0);
                            }

                            unreachable!(
                                "socket read event awakened despite no data being available"
                            )
                        };

                        let mut read_idx = regular.read_idx;
                        let mut total_read = 0;

                        let read = cmp::min(data.len(), buf.len() - read_idx);
                        data[..read].copy_from_slice(&buf[read_idx..read_idx + read]);
                        read_idx += read;
                        total_read += read;

                        if read_idx == buf.len() {
                            regular.read_idx = 0;
                        } else {
                            regular.read_idx = read_idx;
                            regular.recv_buf.push_front(buf);
                        }

                        if regular.recv_buf.is_empty() && !*peer_closed {
                            state.lower_polled(read_polled);
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::Iovec(data) => {
                        let Some(buf) = regular.recv_buf.pop_front() else {
                            if *peer_closed {
                                return Outcome::Success(0);
                            }

                            unreachable!(
                                "socket read event awakened despite no data being available"
                            )
                        };

                        let mut read_idx = regular.read_idx;
                        let mut total_read = 0;

                        for s in data.iter_mut() {
                            let read = cmp::min(s.len(), buf.len() - read_idx);
                            s[..read].copy_from_slice(&buf[read_idx..read_idx + read]);
                            read_idx += read;
                            total_read += read;
                        }

                        if read_idx == buf.len() {
                            regular.read_idx = 0;
                        } else {
                            regular.read_idx = read_idx;
                            regular.recv_buf.push_front(buf);
                        }

                        if regular.recv_buf.is_empty() && !*peer_closed {
                            state.lower_polled(read_polled);
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::File(_data) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        // TODO: blocking incorrectly handled here (see the MSG_WAITFORONE flag in `man 2 recvmmsg`)

                        let mut msg_count = 0;
                        for out_msg in out_msgs.iter_mut() {
                            let Some(buf) = regular.recv_buf.pop_front() else {
                                if !*peer_closed {
                                    state.lower_polled(read_polled);
                                }

                                debug_assert!(
                                    msg_count > 0,
                                    "socket read event awakened despite no data being available"
                                );
                                return Outcome::Success(msg_count);
                            };

                            *out_msg.msg_flags = SocketMsgFlags::EOR;

                            *out_msg.addrlen = sockaddr.encode(out_msg.addr_bytes) as u32;
                            *out_msg.control_len = 0; // TODO: encode ancillary

                            let mut read_idx = regular.read_idx;

                            let mut total_read = 0;
                            for s in out_msg.buf.iter_mut() {
                                let read = cmp::min(buf.len() - read_idx, s.len());
                                s[..read].copy_from_slice(&buf[read_idx..read_idx + read]);
                                read_idx += read;
                                total_read += read;
                            }

                            if read_idx == buf.len() {
                                regular.read_idx = 0;
                            } else {
                                regular.read_idx = read_idx;
                                regular.recv_buf.push_front(buf);
                            }

                            *out_msg.buflen = total_read as u32;

                            msg_count += 1;
                        }

                        if regular.recv_buf.is_empty() && !*peer_closed {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(msg_count)
                    }
                }
            }
            (
                SocketReadState::Finish(_poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::Feedback(_feedback),
                    ..
                }),
            ) => {
                unimplemented!()
                /*

                let buf = feedback.buf.clone();

                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    ReadData::Basic(out_data) => {
                        let mut idx = 0;
                        for s in out_data.iter_mut() {
                            let read = cmp::min(s.len(), buf.borrow().len() - idx);
                            s.copy_from_slice(&buf.borrow().data()[idx..idx + read]);
                            idx += read;
                        }

                        Outcome::Success(idx)
                    },
                    ReadData::File(_) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, socket_flags) => {
                        // TODO: blocking incorrectly handled here (see the MSG_WAITFORONE flag in `man 2 recvmmsg`)

                        let mut msg_count = 0;
                        for out_msg in out_msgs.iter_mut() {
                            *out_msg.msg_flags = SocketMsgFlags::empty();

                            *out_msg.addrlen = 0; // TODO: add peer addr here
                            *out_msg.control_len = 0; // TODO: add ancillary data

                            let mut total_read = 0;
                            for s in out_msg.buf.iter_mut() {
                                let read = cmp::min(buf.borrow().len() - total_read, s.len());
                                s.copy_from_slice(&buf.borrow().data()[total_read..total_read + read]);
                                total_read += read;
                            }

                            buf.borrow_mut().did_read(total_read);
                            *out_msg.buflen = total_read as u32;

                            msg_count += 1;
                        }

                        Outcome::Success(msg_count)
                    }
                }
                */
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::Plugin(plugin),
                    rem_addr,
                    peer_closed,
                }),
            ) => {
                let read_polled = plugin.borrow().read_polled.clone();

                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                let sockaddr = rem_addr.addr().clone();

                match &mut self.data {
                    ReadData::BasicSlice(data) => {
                        let Some(buf) = plugin.borrow_mut().read_buf.pop_front() else {
                            if *peer_closed {
                                return Outcome::Success(0);
                            }

                            unreachable!(
                                "socket read event awakened despite no data being available"
                            )
                        };

                        let mut read_idx = plugin.borrow().read_idx;
                        let read = cmp::min(data.len(), buf.len() - read_idx);
                        data[..read].copy_from_slice(&buf[read_idx..read_idx + read]);
                        read_idx += read;

                        if read_idx == buf.len() {
                            plugin.borrow_mut().read_idx = 0;
                        } else {
                            plugin.borrow_mut().read_idx = read_idx;
                            plugin.borrow_mut().read_buf.push_front(buf);
                        }

                        if plugin.borrow().read_buf.is_empty() && !*peer_closed {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(read)
                    }
                    ReadData::Iovec(data) => {
                        let Some(buf) = plugin.borrow_mut().read_buf.pop_front() else {
                            if *peer_closed {
                                return Outcome::Success(0);
                            }

                            unreachable!(
                                "socket read event awakened despite no data being available"
                            )
                        };

                        let mut read_idx = plugin.borrow().read_idx;
                        let mut total_read = 0;
                        for s in data.iter_mut() {
                            let read = cmp::min(s.len(), buf.len() - read_idx);
                            s[..read].copy_from_slice(&buf[read_idx..read_idx + read]);
                            read_idx += read;
                            total_read += read;
                        }

                        if read_idx == buf.len() {
                            plugin.borrow_mut().read_idx = 0;
                        } else {
                            plugin.borrow_mut().read_idx = read_idx;
                            plugin.borrow_mut().read_buf.push_front(buf);
                        }

                        if plugin.borrow().read_buf.is_empty() && !*peer_closed {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::File(_data) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        // TODO: blocking incorrectly handled here (see the MSG_WAITFORONE flag in `man 2 recvmmsg`)

                        if *peer_closed && rem_addr.protocol() == TransportProtocol::Sctp {
                            let shutdown_msg = SctpShutdownEvent {
                                sse_type: SCTP_SHUTDOWN_EVENT,
                                sse_flags: 0,
                                sse_length: 0,
                                sse_assoc_id: 0,
                            };

                            let shutdown_data = unsafe {
                                slice::from_raw_parts(
                                    (ptr::from_ref(&shutdown_msg)).cast::<u8>(),
                                    mem::size_of_val(&shutdown_msg),
                                )
                            };

                            *out_msgs[0].addrlen = sockaddr.encode(out_msgs[0].addr_bytes) as u32;
                            *out_msgs[0].control_len = 0;
                            *out_msgs[0].msg_flags = SocketMsgFlags::NOTIFICATION;

                            let mut written = 0;
                            for slice in out_msgs[0].buf.iter_mut() {
                                let to_write = cmp::min(shutdown_data.len() - written, slice.len());
                                slice[..to_write]
                                    .copy_from_slice(&shutdown_data[written..written + to_write]);
                                written += to_write;
                            }
                            *out_msgs[0].buflen = shutdown_data.len() as u32;
                            return Outcome::Success(1);
                        }

                        let mut msg_count = 0;
                        for out_msg in out_msgs.iter_mut() {
                            let Some(buf) = plugin.borrow_mut().read_buf.pop_front() else {
                                if !*peer_closed {
                                    debug_assert!(
                                        msg_count > 0,
                                        "socket read event awakened despite no data being available"
                                    );
                                }

                                break
                            };

                            *out_msg.msg_flags = SocketMsgFlags::EOR;

                            *out_msg.addrlen = sockaddr.encode(out_msg.addr_bytes) as u32;
                            *out_msg.control_len = 0; // TODO: encode ancillary

                            let mut read_idx = plugin.borrow().read_idx;
                            let mut total_read = 0;
                            for s in out_msg.buf.iter_mut() {
                                let read = cmp::min(buf.len() - read_idx, s.len());
                                s[..read].copy_from_slice(&buf[read_idx..read_idx + read]);
                                read_idx += read;
                                total_read += read;
                            }

                            if read_idx == buf.len() {
                                plugin.borrow_mut().read_idx = 0;
                            } else {
                                plugin.borrow_mut().read_idx = read_idx;
                                plugin.borrow_mut().read_buf.push_front(buf);
                            }

                            *out_msg.buflen = total_read as u32;

                            msg_count += 1;
                        }

                        if plugin.borrow().read_buf.is_empty() && !*peer_closed {
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(msg_count)
                    }
                }
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::Sink,
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                Outcome::Success(0)
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::NullSink,
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    ReadData::BasicSlice(s) => {
                        s.fill(0);

                        Outcome::Success(s.len())
                    }
                    ReadData::Iovec(out_slices) => {
                        let mut total_read = 0;
                        for s in out_slices.iter_mut() {
                            for b in s.iter_mut() {
                                *b = 0;
                            }
                            total_read += s.len();
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::File(_) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        for msg in out_msgs.iter_mut() {
                            *msg.addrlen = 0;
                            *msg.control_len = 0;
                            for s in msg.buf.iter_mut() {
                                for b in s.iter_mut() {
                                    *b = 0;
                                }
                            }
                        }

                        Outcome::Success(out_msgs.len())
                    }
                }
            }
            (
                SocketReadState::Finish(poller_id),
                SocketState::Connected(ConnectedSocket {
                    backend: ConnectedBackend::Fuzz(endpoint),
                    ..
                }),
            ) => {
                if let Some(poller) = poller_id.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    ReadData::BasicSlice(s) => {
                        let read = cmp::min(
                            s.len(),
                            state.global.fuzz_input.len() - endpoint.borrow().read_idx,
                        );
                        s.copy_from_slice(
                            &state.global.fuzz_input
                                [endpoint.borrow().read_idx..endpoint.borrow().read_idx + read],
                        );
                        endpoint.borrow_mut().read_idx += read;

                        if endpoint.borrow().read_idx == state.global.fuzz_input.len() {
                            let read_polled = endpoint.borrow().read_polled.clone();
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(read)
                    }
                    ReadData::Iovec(out_slices) => {
                        let mut total_read = 0;
                        for s in out_slices.iter_mut() {
                            let read = cmp::min(
                                s.len(),
                                state.global.fuzz_input.len() - endpoint.borrow().read_idx,
                            );
                            s.copy_from_slice(
                                &state.global.fuzz_input
                                    [endpoint.borrow().read_idx..endpoint.borrow().read_idx + read],
                            );
                            endpoint.borrow_mut().read_idx += read;
                            total_read += read;
                        }

                        if endpoint.borrow().read_idx == state.global.fuzz_input.len() {
                            let read_polled = endpoint.borrow().read_polled.clone();
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(total_read)
                    }
                    ReadData::File(_) => Outcome::Error(Errno::ESPIPE),
                    ReadData::Socket(out_msgs, _socket_flags) => {
                        let mut total_read = 0;
                        for out_msg in out_msgs.iter_mut() {
                            for s in out_msg.buf.iter_mut() {
                                let read = cmp::min(
                                    s.len(),
                                    state.global.fuzz_input.len() - endpoint.borrow().read_idx,
                                );
                                s.copy_from_slice(
                                    &state.global.fuzz_input[endpoint.borrow().read_idx
                                        ..endpoint.borrow().read_idx + read],
                                );
                                endpoint.borrow_mut().read_idx += read;
                                total_read += read;
                            }
                        }

                        if endpoint.borrow().read_idx == state.global.fuzz_input.len() {
                            let read_polled = endpoint.borrow().read_polled.clone();
                            state.lower_polled(&read_polled);
                        }

                        Outcome::Success(total_read)
                    }
                }
            }
            _ => Outcome::Error(Errno::EINVAL), // Invalid socket state
        }
    }
}

pub enum SocketWriteState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

pub struct SocketWriteEvent<'a> {
    socket: GlobalRc<SocketInfo>,
    nonblocking: bool,
    data: WriteData<'a>,
    state: SocketWriteState,
}

impl<'a> SocketWriteEvent<'a> {
    #[inline]
    pub fn new(socket: GlobalRc<SocketInfo>, nonblocking: bool, data: WriteData<'a>) -> Self {
        Self {
            socket,
            nonblocking,
            data,
            state: SocketWriteState::Start,
        }
    }
}

impl Event for SocketWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let mut socket_info = self.socket.clone();

        let local_addr = get_or_assign_local(&mut socket_info, state);
        let mut borrowed_socket_info = socket_info.borrow_mut();

        match (&self.state, &mut borrowed_socket_info.state) {
            (SocketWriteState::Start, SocketState::Connectionless(_conn)) => {
                // Sending out on a Connectionless socket doesn't require polling--lossy packets are simply dropped
                self.state = SocketWriteState::Finish(None);
                Outcome::Yield(YieldUntil::Immediate)
            }
            (SocketWriteState::Start, SocketState::Connected(conn)) => {
                let write_polled = conn.write_polled();

                if let Some(write_polled) = write_polled.as_ref() {
                    if state.polled_is_ready(&write_polled) {
                        self.state = SocketWriteState::Finish(None);
                        Outcome::Yield(YieldUntil::Immediate)
                    } else if self.nonblocking {
                        Outcome::Error(Errno::EAGAIN)
                    } else {
                        let poller_id = state.new_poller();
                        state.register_poller(poller_id.clone(), write_polled.clone());

                        self.state = SocketWriteState::Finish(Some(poller_id));
                        Outcome::Yield(YieldUntil::None)
                    }
                } else {
                    self.state = SocketWriteState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                }
            }
            (SocketWriteState::Finish(poller), SocketState::Connectionless(conn)) => {
                if let Some(poller) = poller.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    WriteData::BasicSlice(s) => {
                        let mut data = Vec::new_in(fizzle_alloc());
                        data.extend_from_slice(s);

                        let Some(peer) = conn.dst_socket(state) else {
                            log::warn!("no destination for connectionless socket information to be received--dropping sent packet");
                            return Outcome::Success(s.len());
                        };

                        drop(borrowed_socket_info); // In case the peer is the same as this socket
                        let mut peer_ref = peer.borrow_mut();
                        let SocketState::Connectionless(peer_conn) = &mut peer_ref.state else {
                            unreachable!()
                        };

                        match &mut peer_conn.backend {
                            ConnectionlessBackend::Peered(p) => {
                                p.recv_buf.push_back(ConnectionlessMessage {
                                    source: local_addr.addr().clone(),
                                    ancillary: Vec::new_in(fizzle_alloc()), // TODO: add ancillary here,
                                    data,
                                });
                            }
                            ConnectionlessBackend::Feedback(f) => {
                                f.feedback_buf.push_back((
                                    ConnectionlessMessage {
                                        source: local_addr.addr().clone(),
                                        ancillary: Vec::new_in(fizzle_alloc()), // TODO: add ancillary here,
                                        data,
                                    },
                                    self.socket.clone(),
                                ));
                            }
                            ConnectionlessBackend::Plugin(p) => {
                                let mut plugin_ref = p.borrow_mut();
                                plugin_ref.write_buf.push_back(data);
                            }
                            ConnectionlessBackend::Passthrough => unimplemented!(),
                            _ => (),
                        }

                        Outcome::Success(s.len())
                    }
                    WriteData::Iovec(v) => {
                        let full_len = v.iter().map(|s| s.len()).sum::<usize>();
                        let Some(peer) = conn.dst_socket(state) else {
                            log::warn!("no destination for connectionless socket information to be received--dropping sent packet");
                            return Outcome::Success(full_len);
                        };

                        let mut peer_ref = peer.borrow_mut();
                        let SocketState::Connectionless(peer_conn) = &mut peer_ref.state else {
                            unreachable!()
                        };

                        let mut data = Vec::new_in(fizzle_alloc());
                        for s in v.iter() {
                            data.extend_from_slice(s);
                        }

                        match &mut peer_conn.backend {
                            ConnectionlessBackend::Peered(p) => {
                                p.recv_buf.push_back(ConnectionlessMessage {
                                    source: local_addr.addr().clone(),
                                    ancillary: Vec::new_in(fizzle_alloc()), // TODO: add ancillary here,
                                    data,
                                });
                            }
                            ConnectionlessBackend::Feedback(f) => {
                                f.feedback_buf.push_back((
                                    ConnectionlessMessage {
                                        source: local_addr.addr().clone(),
                                        ancillary: Vec::new_in(fizzle_alloc()), // TODO: add ancillary here,
                                        data,
                                    },
                                    self.socket.clone(),
                                ));
                            }
                            ConnectionlessBackend::Plugin(p) => {
                                let mut plugin_ref = p.borrow_mut();
                                plugin_ref.write_buf.push_back(data);
                            }
                            ConnectionlessBackend::Passthrough => unimplemented!(),
                            _ => (),
                        }

                        Outcome::Success(full_len)
                    }
                    WriteData::File(_) => return Outcome::Error(Errno::ESPIPE),
                    WriteData::Socket(s, _) => {
                        let conn_addr = match conn.dst_socket(state) {
                            Some(mut peer) => Some(get_or_assign_local(&mut peer, state)),
                            None => None,
                        };

                        let mut num_written = 0;
                        let mut write_error: Errno = Errno::SUCCESS;

                        let rem_addr = conn.rem_addr.clone();
                        drop(borrowed_socket_info); // In case the peer is the same as this socket

                        for write_data in s.iter_mut() {
                            let addr = match &conn_addr {
                                Some(_) if write_data.addr_bytes.is_some() => {
                                    write_error = Errno::EISCONN;
                                    continue;
                                }
                                Some(addr) => addr.clone(),
                                None => {
                                    let sockaddr = match &write_data.addr_bytes {
                                        Some(addr_bytes) => {
                                            let Ok(sockaddr) = SockAddr::decode(addr_bytes) else {
                                                log::warn!("bad address passed to socket");
                                                write_error = Errno::EINVAL;
                                                continue;
                                            };
                                            log::debug!("Sending data to {:?}", sockaddr);
                                            sockaddr
                                        }
                                        None => match &rem_addr {
                                            Some(transp_addr) => transp_addr.addr().clone(),
                                            None => {
                                                write_error = Errno::ENOTCONN;
                                                continue;
                                            }
                                        }
                                    };

                                    TransportAddress {
                                        sockaddr,
                                        protocol: local_addr.protocol,
                                    }
                                }
                            };

                            let Some(dst_info) = state.global.socket_locations.get_mut(&addr)
                            else {
                                // write_error = Errno::EHOSTUNREACH;
                                *write_data.buflen =
                                    write_data.buf.iter().map(|s| s.len()).sum::<usize>() as u32;
                                num_written += 1;
                                continue;
                            };

                            let Some(dst_socket) = dst_info.next_bound() else {
                                // write_error = Errno::EHOSTUNREACH;
                                *write_data.buflen =
                                    write_data.buf.iter().map(|s| s.len()).sum::<usize>() as u32;
                                num_written += 1;
                                continue;
                            };

                            let mut dst_socket_mut = dst_socket.borrow_mut();
                            let SocketState::Connectionless(peer_conn) = &mut dst_socket_mut.state
                            else {
                                unreachable!()
                            };

                            let mut ancillary = Vec::new_in(fizzle_alloc());
                            ancillary.extend_from_slice(write_data.control_info);

                            let mut data = Vec::new_in(fizzle_alloc());
                            for s in write_data.buf.iter() {
                                data.extend_from_slice(s);
                            }

                            match &mut peer_conn.backend {
                                ConnectionlessBackend::Peered(p) => {
                                    p.recv_buf.push_back(ConnectionlessMessage {
                                        source: local_addr.addr().clone(),
                                        ancillary,
                                        data,
                                    });
                                }
                                ConnectionlessBackend::Feedback(f) => {
                                    f.feedback_buf.push_back((
                                        ConnectionlessMessage {
                                            source: local_addr.addr().clone(),
                                            ancillary,
                                            data,
                                        },
                                        self.socket.clone(),
                                    ));
                                }
                                ConnectionlessBackend::Plugin(p) => {
                                    let mut plugin_ref = p.borrow_mut();
                                    plugin_ref.write_buf.push_back(data);
                                }
                                ConnectionlessBackend::Passthrough => unimplemented!(),
                                _ => *write_data.buflen = data.len().try_into().unwrap(),
                            }

                            num_written += 1;
                        }

                        if num_written > 0 {
                            Outcome::Success(num_written)
                        } else {
                            Outcome::Error(write_error)
                        }
                    }
                }
            }
            (SocketWriteState::Finish(poller), SocketState::Connected(conn)) => {
                if let Some(poller) = poller.as_ref() {
                    state.delete_poller(poller.clone());
                }

                match &mut self.data {
                    WriteData::BasicSlice(s) => {
                        let mut data = Vec::new_in(fizzle_alloc());
                        data.extend_from_slice(s);

                        match &mut conn.backend {
                            ConnectedBackend::Peered(p) => {
                                let Some(peer) = p.peer.upgrade() else {
                                    return Outcome::Success(0);
                                };

                                let mut peer_mut = peer.borrow_mut();
                                let SocketState::Connected(conn) = &mut peer_mut.state else {
                                    unreachable!()
                                };

                                let ConnectedBackend::Peered(peer_conn) = &mut conn.backend else {
                                    unreachable!()
                                };

                                (ConnectionlessMessage {
                                    source: local_addr.addr().clone(),
                                    ancillary: Vec::new_in(fizzle_alloc()), // TODO: add ancillary here,
                                    data,
                                });

                                let read_polled = peer_conn.read_polled.clone();
                                state.raise_polled(&read_polled);
                            }
                            ConnectedBackend::Feedback(_f) => {
                                unimplemented!()
                            }
                            ConnectedBackend::Plugin(p) => {
                                let mut plugin_ref = p.borrow_mut();
                                plugin_ref.write_buf.push_back(data);
                            }
                            ConnectedBackend::Passthrough => unimplemented!(),
                            _ => (),
                        }

                        Outcome::Success(s.len())
                    }
                    WriteData::Iovec(v) => {
                        let mut data = Vec::new_in(fizzle_alloc());
                        for s in v.iter() {
                            data.extend_from_slice(s);
                        }
                        let total_len = data.len();

                        match &mut conn.backend {
                            ConnectedBackend::Peered(p) => {
                                let Some(peer) = p.peer.upgrade() else {
                                    return Outcome::Success(0);
                                };

                                let mut peer_mut = peer.borrow_mut();
                                let SocketState::Connected(conn) = &mut peer_mut.state else {
                                    unreachable!()
                                };

                                let ConnectedBackend::Peered(peer_conn) = &mut conn.backend else {
                                    unreachable!()
                                };

                                (ConnectionlessMessage {
                                    source: local_addr.addr().clone(),
                                    ancillary: Vec::new_in(fizzle_alloc()), // TODO: add ancillary here,
                                    data,
                                });

                                let read_polled = peer_conn.read_polled.clone();
                                state.raise_polled(&read_polled);
                            }
                            ConnectedBackend::Feedback(_f) => {
                                unimplemented!()
                            }
                            ConnectedBackend::Plugin(p) => {
                                let mut plugin_ref = p.borrow_mut();
                                plugin_ref.write_buf.push_back(data);
                            }
                            ConnectedBackend::Passthrough => unimplemented!(),
                            _ => (),
                        }

                        Outcome::Success(total_len)
                    }
                    WriteData::File(_) => return Outcome::Error(Errno::ESPIPE),
                    WriteData::Socket(s, _) => {
                        for write_data in s.iter_mut() {
                            let mut data = Vec::new_in(fizzle_alloc());
                            for s in write_data.buf.iter() {
                                data.extend_from_slice(s);
                            }

                            let mut ancillary = Vec::new_in(fizzle_alloc());
                            ancillary.extend_from_slice(write_data.control_info);

                            match &mut conn.backend {
                                ConnectedBackend::Peered(p) => {
                                    let Some(peer) = p.peer.upgrade() else {
                                        return Outcome::Success(0);
                                    };

                                    let mut peer_mut = peer.borrow_mut();
                                    let SocketState::Connected(conn) = &mut peer_mut.state else {
                                        unreachable!()
                                    };

                                    let ConnectedBackend::Peered(peer_conn) = &mut conn.backend
                                    else {
                                        unreachable!()
                                    };

                                    *write_data.buflen = data.len() as u32;
                                    (ConnectionlessMessage {
                                        source: local_addr.addr().clone(),
                                        ancillary,
                                        data,
                                    });

                                    let read_polled = peer_conn.read_polled.clone();
                                    state.raise_polled(&read_polled);
                                }
                                ConnectedBackend::Feedback(_f) => {
                                    unimplemented!()
                                }
                                ConnectedBackend::Plugin(p) => {
                                    let mut plugin_ref = p.borrow_mut();
                                    *write_data.buflen = data.len() as u32;
                                    plugin_ref.write_buf.push_back(data);
                                }
                                ConnectedBackend::Passthrough => unimplemented!(),
                                _ => (),
                            }
                        }

                        Outcome::Success(s.len())
                    }
                }
            }
            _ => Outcome::Error(Errno::EINVAL), // Invalid socket state
        }
    }
}
