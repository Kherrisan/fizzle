//! Process I/O shims.
//!
//!

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::{array, mem};

use crate::hook_macros;
use crate::state::fd::{FdInfo, FdResource};
use crate::state::identifiers::{DescriptorId, SocketId};
use crate::state::{
    ConnectedPeer, ConnectedSocket, ConnectingSocket, FizzleContext, IoBackend, PendingInfo, PluginInfo, PolledInfo, PolledItem, ServerSocket, SocketLocationInfo, SocketState, UnassociatedSocket
};
use fizzle_common::io::{AddressFamily, TransportAddress, TransportProtocol};
use fizzle_common::storage::RingBuffer;
use heapless::spsc::Queue;

pub struct SockAddrError;

unsafe fn decode_inet_address(
    addr: *const libc::sockaddr,
    addrlen: libc::socklen_t,
) -> Result<SocketAddr, SockAddrError> {
    match (*addr).sa_family as i32 {
        libc::AF_INET => {
            let addr = addr as *const libc::sockaddr_in;
            if addrlen as usize != mem::size_of::<libc::sockaddr_in>() {
                return Err(SockAddrError);
            }

            // TODO: verify correctness of these conversions
            let addr_bytes = u32::from_be((*addr).sin_addr.s_addr).to_be_bytes();
            let port = u16::from_be((*addr).sin_port);
            Ok(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(addr_bytes[0], addr_bytes[1], addr_bytes[2], addr_bytes[3]),
                port,
            )))
        }
        libc::AF_INET6 => {
            let addr = addr as *const libc::sockaddr_in6;
            if addrlen as usize != mem::size_of::<libc::sockaddr_in6>() {
                return Err(SockAddrError);
            }

            // TODO: verify correctness of these conversions
            let addr_segments: [u16; 8] = array::from_fn(|i| {
                u16::from_be_bytes(
                    (*addr).sin6_addr.s6_addr[2 * i..(2 * i) + 2]
                        .try_into()
                        .unwrap(),
                )
            }); // TODO: replace with newer libc functions when they arrive
            let port = u16::from_be((*addr).sin6_port);
            let flow_info = u32::from_be((*addr).sin6_flowinfo);
            let scope_id = u32::from_be((*addr).sin6_scope_id);
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::new(
                    addr_segments[0],
                    addr_segments[1],
                    addr_segments[2],
                    addr_segments[3],
                    addr_segments[4],
                    addr_segments[5],
                    addr_segments[6],
                    addr_segments[7],
                ),
                port,
                flow_info,
                scope_id,
            )))
        }
        _ => panic!(
            "fizzle does not currently support address family {}",
            (*addr).sa_family
        ),
    }
}

///
/// # Safety
///
/// It is the responsibility of the caller to ensure that `addr` points to valid bytes that are
/// sized according to the address family in the address (e.g., the address length for an `AF_INET`
/// sockaddr should be equal to `mem::size_of::<libc::sockaddr_in>()`).
unsafe fn encode_inet_address(addr: *mut libc::sockaddr, address: &SocketAddr) {
    match address {
        SocketAddr::V4(v4) => {
            let addr = addr as *mut libc::sockaddr_in;
            (*addr).sin_addr.s_addr = u32::from_be_bytes(v4.ip().octets()).to_be();
            (*addr).sin_port = v4.port().to_be();
        }
        SocketAddr::V6(v6) => {
            let addr = addr as *mut libc::sockaddr_in6;
            (*addr).sin6_addr.s6_addr = v6.ip().octets();
            (*addr).sin6_port = v6.port().to_be();
            (*addr).sin6_flowinfo = v6.flowinfo().to_be();
            (*addr).sin6_scope_id = v6.scope_id().to_be();
        }
    }
}

hook_macros::hook! {
    unsafe fn socket(
        domain: libc::c_int,
        socktype: libc::c_int,
        protocol: libc::c_int
    ) -> libc::c_int => fizzle_socket(ctx) {

        let fd = hook_macros::real!(socket)(domain, socktype, protocol);
        if fd < 0 {
            return fd
        }

        let nonblocking = (socktype & libc::SOCK_NONBLOCK) != 0;
        let close_on_exec = (socktype & libc::SOCK_CLOEXEC) != 0;

        let family = match domain {
            libc::AF_INET => AddressFamily::Ipv4,
            libc::AF_INET6 => AddressFamily::Ipv6,
            _ => panic!("unsupported socket address family {}",  domain),
        };

        let protocol = match protocol {
            libc::IPPROTO_TCP => TransportProtocol::Tcp,
            libc::IPPROTO_SCTP => TransportProtocol::Sctp,
            libc::IPPROTO_UDP => TransportProtocol::Udp,
            _ => panic!("unsupported transport protocol {}", protocol),
        };

        let socket_id = ctx.global().sockets.put(SocketState::Unassociated(UnassociatedSocket {
            local_addr: None,
            family,
            protocol,
        }));

        ctx.local().fds.insert(DescriptorId::new(fd), FdInfo {
            close_on_exec,
            nonblocking,
            resource: FdResource::Socket(socket_id)
        });

        fd
    }
}

hook_macros::hook! {
    unsafe fn bind(
        fd: libc::c_int,
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::c_int => fizzle_bind(ctx) {

        let descriptor_id = DescriptorId::new(fd);
        let Some(fd_info) = ctx.local().fds.get(descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        // TODO: support AF_UNSPEC?
        let Ok(socket_addr) = decode_inet_address(addr, addrlen) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let transport_addr = match ctx.global().sockets.get(socket_id).unwrap() {
            SocketState::Unassociated(UnassociatedSocket { local_addr: Some(_), .. }) => {
                // Socket is already bound
                *libc::__errno_location() = libc::EINVAL;
                return -1
            },
            SocketState::Unassociated(UnassociatedSocket { protocol: TransportProtocol::Tcp, .. }) => TransportAddress::Tcp(socket_addr),
            SocketState::Unassociated(UnassociatedSocket { protocol: TransportProtocol::Udp, .. }) => TransportAddress::Udp(socket_addr),
            SocketState::Unassociated(UnassociatedSocket { protocol: TransportProtocol::Sctp, .. }) => TransportAddress::Sctp(socket_addr),
            _ => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            }
        };

        match ctx.global().socket_locations.entry(transport_addr) {
            heapless::Entry::Occupied(_) => {
                *libc::__errno_location() = libc::EADDRINUSE;
                -1
            },
            heapless::Entry::Vacant(v) => {
                v.insert(SocketLocationInfo {
                    bound_socket: Some(socket_id),
                    pending: None,
                }).unwrap();

                let SocketState::Unassociated(UnassociatedSocket { local_addr, .. }) = ctx.global().sockets.get_mut(socket_id).unwrap() else {
                    panic!("internal state error in fizzle--unreachable code reached");
                };
                *local_addr = Some(transport_addr);

                0
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn listen(
        fd: libc::c_int,
        _backlog: libc::c_int
    ) -> libc::c_int => fizzle_listen(ctx) {

        let descriptor_id = DescriptorId::new(fd);
        let Some(fd_info) = ctx.local().fds.get(descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        let SocketState::Unassociated(socket_info) = ctx.global().sockets.get_mut(socket_id).unwrap() else {
            panic!("internal fizzle error--`listen()` called on socket in unexpected state");
        };

        // Connectionless protocols shouldn't `listen()`
        if socket_info.protocol == TransportProtocol::Udp {
            *libc::__errno_location() = libc::EOPNOTSUPP;
            return -1
        }

        let local_addr = match socket_info.local_addr {
            Some(addr) => addr,
            None => {
                let family = socket_info.family;
                let protocol = socket_info.protocol;

                let addr = ctx.global().next_ephemeral_address(family, protocol);
                if ctx.global().socket_locations.contains_key(&addr) {
                    *libc::__errno_location() = libc::EADDRINUSE;
                    return -1
                }

                ctx.global().socket_locations.insert(addr, SocketLocationInfo {
                    bound_socket: Some(socket_id),
                    pending: None,
                }).unwrap();

                addr
            }
        };

        // Allocate server context and set up polling
        let ready_to_connect = ctx.global().polled_events.put(PolledInfo::new(PolledItem::None));
        *ctx.global().sockets.get_mut(socket_id).unwrap() = SocketState::Server(ServerSocket {
            backend: None,
            local_addr,
            connecting: Queue::new(),
            ready_to_connect,
        });

        ctx.global().polled_events.get_mut(ready_to_connect).unwrap().polled_item = PolledItem::Socket(socket_id);

        0
    }
}

hook_macros::hook! {
    unsafe fn connect(
        fd: libc::c_int,
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::c_int => fizzle_connect(ctx) {

        let descriptor_id = DescriptorId::new(fd);
        let Some(fd_info) = ctx.local().fds.get(descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking;

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        // TODO: support AF_UNSPEC?
        let Ok(socket_addr) = decode_inet_address(addr, addrlen) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        match ctx.global().sockets.get(socket_id).unwrap() {
            SocketState::Unassociated(sock) => {
                let protocol = sock.protocol;
                let family = sock.family;
                let local_addr = match sock.local_addr {
                    Some(addr) => addr,
                    None => ctx.global().next_ephemeral_address(family, protocol),
                };
                let rem_addr = match protocol {
                    TransportProtocol::Tcp => TransportAddress::Tcp(socket_addr),
                    TransportProtocol::Udp => TransportAddress::Udp(socket_addr),
                    TransportProtocol::Sctp => TransportAddress::Sctp(socket_addr),
                };

                // First: is the address currently bound?
                let Some(SocketLocationInfo { bound_socket: Some(server_socket_id), .. }) = ctx.global().socket_locations.get(&rem_addr) else {
                    // No socket is bound to the given address...
                    // TODO: support catch-all addresses (0.0.0.0)
                    // TODO: maybe wait until a server does exist at the location??
                    *libc::__errno_location() = libc::ECONNREFUSED;
                    return -1;
                };

                let server_socket_id = *server_socket_id;
                let SocketState::Server(server_info) = ctx.global().sockets.get_mut(server_socket_id).unwrap() else {
                    *libc::__errno_location() = libc::ECONNREFUSED;
                    return -1 // TODO: in the future, wait until a server does exist
                };

                if let Some(backend) = server_info.backend {
                    // Mark the socket as connected immediately, since it's connecting to a backend
                    // TODO: some programs may be confused by this--a connection immediately returning 0 is unusual for a transport protocol

                    // Duplicate the backend--we can't use the same IDs as the server, since this is a new connection
                    let backend = match backend {
                        IoBackend::Feedback(_) => {
                            let new_buffer_id = ctx.global().buffers.put(RingBuffer::default());
                            IoBackend::Feedback(new_buffer_id)
                        },
                        IoBackend::Plugin(plugin_id) => {
                            // Create new plugin
                            let connect_plugin_id = ctx.global().plugins.put(PluginInfo { endpoint: todo!(), stream: todo!(), input: todo!(), in_polled: todo!(), output: todo!(), out_polled: todo!(), module: todo!() });
                        },
                        IoBackend::Sink => IoBackend::Sink,
                        IoBackend::NullSink => IoBackend::NullSink,
                        IoBackend::Fuzz => IoBackend::Fuzz,
                    };

                    match ctx.global().sockets.get_mut(socket_id).unwrap() {
                        SocketState::Connecting(connecting_info) => {
                            let polled = connecting_info.polled; // Delete the current polled instance TODO: fix to remove dangling refs
                            ctx.global().polled_events.remove(polled).unwrap();
                        },
                        _ => panic!("internal fizzle error"),
                    }

                    let connect_polled = ctx.global().polled_events.put(PolledInfo::new(PolledItem::Socket(socket_id)));
                    let connect_recv_buf = ctx.global().buffers.put(RingBuffer::new());

                    *ctx.global().sockets.get_mut(socket_id).unwrap() = SocketState::Connected(ConnectedSocket {
                        local_addr,
                        peer: ConnectedPeer::Emulated(backend),
                        polled: connect_polled,
                        recv_buf: connect_recv_buf,
                    });

                    0
                } else {
                    let Ok(_) = server_info.connecting.enqueue(socket_id) else {
                        *libc::__errno_location() = libc::ECONNREFUSED;
                        return -1
                    };

                    let server_poll = server_info.ready_to_connect;
                    ctx.raise_polled_event(server_poll);

                    let client_poll = ctx.global().polled_events.put(PolledInfo::new(PolledItem::Socket(socket_id)));
                    *ctx.global().sockets.get_mut(socket_id).unwrap() = SocketState::Connecting(ConnectingSocket {
                        backend: None,
                        polled: client_poll,
                        local_addr,
                        rem_addr
                    });

                    if is_nonblocking {
                        *libc::__errno_location() = libc::EINPROGRESS;
                        -1
                    } else {
                        ctx.yield_poll_once(client_poll);
                        0
                    }
                }
            },
            SocketState::Server(_) => {
                panic!("unexpected fizzle state--`connect()` called on listening socket")
            },
            SocketState::PendingConnection(_) => panic!("unexpected fizzle state--PendingConnection had `connect` called on it"),
            SocketState::Connecting(_) => if is_nonblocking {
                *libc::__errno_location() = libc::EALREADY;
                -1
            } else {
                ctx.yield_thread(); // TODO: more work to be done here... (see above)
                0
            }
            SocketState::Connected(_) => {
                *libc::__errno_location() = libc::EISCONN;
                -1
            }
            SocketState::Error => {
                *libc::__errno_location() = libc::ECONNREFUSED;
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn accept(
        fd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_accept(_ctx) {
        fizzle_accept4(fd, addr, addrlen, 0)
    }
}

hook_macros::hook! {
    unsafe fn accept4(
        fd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_accept4(ctx) {

        if !addr.is_null() {
            if (*addr).sa_family as i32 == libc::AF_INET && (*addrlen) as usize != mem::size_of::<libc::sockaddr_in>()
                    || (*addr).sa_family as i32 == libc::AF_INET6 && (*addrlen) as usize != mem::size_of::<libc::sockaddr_in6>() {
                *libc::__errno_location() = libc::EINVAL;
                return -1;
            }
        }

        let descriptor_id = DescriptorId::new(fd);
        let Some(fd_info) = ctx.local().fds.get(descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let is_nonblocking = fd_info.nonblocking;

        let FdResource::Socket(server_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        let SocketState::Server(server_info) = ctx.global().sockets.get_mut(server_id).unwrap() else {
            *libc::__errno_location() = libc::EINVAL;
            return -1;
        };

        let server_addr = server_info.local_addr;
        let ready_to_connect = server_info.ready_to_connect;

        let bound_info = ctx.global().socket_locations.get(&server_addr).unwrap();
        if let Some(PendingInfo { client, poll }) = bound_info.pending {
            let SocketState::PendingConnection(pending_info) = ctx.global().sockets.get_mut(client).unwrap() else {
                panic!("unexpected fizzle state--pending socket ID not in PendingConnection state");
            };

            // Update the linked list of pending clients
            match pending_info.next_pending {
                Some(pending_id) => ctx.global().socket_locations.get_mut(&server_addr).unwrap().pending.as_mut().unwrap().client = pending_id,
                None => ctx.global().socket_locations.get_mut(&server_addr).unwrap().pending = None,
            }

            ctx.raise_polled_event(poll);

            return join_socket_pair(&mut ctx, server_id, client, flags)
        }

        let SocketState::Server(server_info) = ctx.global().sockets.get_mut(server_id).unwrap() else {
            *libc::__errno_location() = libc::EINVAL;
            return -1;
        };

        if let Some(connecting_id) = server_info.connecting.dequeue() {
            let SocketState::Connecting(connecting_info) = ctx.global().sockets.get(connecting_id).unwrap() else {
                panic!("unexpected fizzle internal state--socket in server connecting queue was not `Connecting` variant")
            };

            if !addr.is_null() {
                encode_inet_address(addr, connecting_info.local_addr.address());
            }

            let polled = connecting_info.polled;
            ctx.raise_polled_event(polled);

            return join_socket_pair(&mut ctx, server_id, connecting_id, flags);

        } else if is_nonblocking {
            *libc::__errno_location() = libc::EAGAIN; // or EWOULDBLOCK
            return -1

        } else { // !is_nonblocking
            ctx.yield_poll_once(ready_to_connect);

            // Now there's a connected socket ready
            let SocketState::Server(server_info) = ctx.global().sockets.get_mut(server_id).unwrap() else {
                panic!("internal fizzle error")
            };

            let connecting_id = server_info.connecting.dequeue().unwrap();
            let SocketState::Connecting(connecting_info) = ctx.global().sockets.get(connecting_id).unwrap() else {
                panic!("unexpected fizzle internal state--socket in server connecting queue was not `Connecting` variant")
            };

            // Write the remote address of the connecting socket for the accept
            if !addr.is_null() {
                encode_inet_address(addr, connecting_info.local_addr.address());
            }

            return join_socket_pair(&mut ctx, server_id, connecting_id, flags);
        }
    }
}

/// Helper function for `accept()`--creates two connected sockets based on a connecting and server socket and returns both.
fn join_socket_pair(
    ctx: &mut FizzleContext,
    server_id: SocketId,
    connecting_id: SocketId,
    flags: libc::c_int,
) -> libc::c_int {
    let local_addr = match ctx.global().sockets.get(server_id).unwrap() {
        SocketState::Server(server_info) => server_info.local_addr,
        _ => panic!("internal fizzle state"),
    };

    match ctx.global().sockets.get_mut(connecting_id).unwrap() {
        SocketState::Connecting(connecting_info) => {
            let polled = connecting_info.polled; // Delete the current polled instance TODO: fix to remove dangling refs
            ctx.global().polled_events.remove(polled).unwrap();
        }
        _ => panic!("internal fizzle error"),
    }

    let accept_recv_buf = ctx.global().buffers.put(RingBuffer::new());
    let accept_polled = ctx
        .global()
        .polled_events
        .put(PolledInfo::new(PolledItem::None));
    let accepted_id = ctx
        .global()
        .sockets
        .put(SocketState::Connected(ConnectedSocket {
            local_addr,
            peer: ConnectedPeer::Socket(connecting_id),
            polled: accept_polled,
            recv_buf: accept_recv_buf,
        }));
    ctx.global()
        .polled_events
        .get_mut(accept_polled)
        .unwrap()
        .polled_item = PolledItem::Socket(accepted_id);

    let connect_polled = ctx
        .global()
        .polled_events
        .put(PolledInfo::new(PolledItem::Socket(connecting_id)));
    let connect_recv_buf = ctx.global().buffers.put(RingBuffer::new());

    *ctx.global().sockets.get_mut(connecting_id).unwrap() =
        SocketState::Connected(ConnectedSocket {
            local_addr,
            peer: ConnectedPeer::Socket(accepted_id),
            polled: connect_polled,
            recv_buf: connect_recv_buf,
        });

    // The two sockets are now joined--add a file descriptor to the accepted socket
    ctx.local().fds.insert(
        DescriptorId::new(crate::alias_fd_create()),
        FdInfo {
            close_on_exec: (flags & libc::O_CLOEXEC) != 0,
            nonblocking: (flags & libc::O_NONBLOCK) != 0,
            resource: FdResource::Socket(accepted_id),
        },
    );

    0 // TODO: need to account for error conditions within this function
}
