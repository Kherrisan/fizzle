//! Process I/O shims.
//!
//!

use std::mem;
use std::net::SocketAddr;

use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::state::backend::{ConnectedBackend, ConnectingBackend, ConnectionlessBackend, IoBackend, RegularConnected, RegularConnectionless, ServerBackend, StandardFeedback};
use crate::state::fd::{FdInfo, FdResource};
use crate::state::identifiers::{DescriptorId, SocketId};
use crate::state::{
    ConnectedSocket, ConnectingSocket, ConnectionlessSocket, FizzleContext, PendingInfo, PolledInfo, ServerSocket, SocketLocationInfo, SocketState, UnassociatedSocket
};
use crate::{decode_inet_address, hook_macros};
use fizzle_common::io::{AddressFamily, TransportAddress, TransportProtocol};
use fizzle_common::storage::Buffer;
use heapless::spsc::Queue;

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

        let socket_id = match protocol {
            0 | libc::IPPROTO_TCP => ctx.global().sockets.put(SocketState::Unassociated(UnassociatedSocket {
                local_addr: None,
                family,
                protocol: TransportProtocol::Tcp,
            })),
            libc::IPPROTO_SCTP => ctx.global().sockets.put(SocketState::Unassociated(UnassociatedSocket {
                local_addr: None,
                family,
                protocol: TransportProtocol::Tcp,
            })),
            libc::IPPROTO_UDP => {
                let local_addr = ctx.global().next_ephemeral_address(family, TransportProtocol::Udp).address().clone();
                let recv_buf = ctx.global().buffers.put(Buffer::new());
                let read_polled = ctx.global().polled_events.put(PolledInfo::new());
                let write_polled = ctx.global().polled_events.put(PolledInfo::new_raised());

                ctx.global().sockets.put(SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Regular(RegularConnectionless {
                        recv_buf,
                        read_polled,
                        write_polled,
                    }),
                    local_addr,
                    rem_addr: None,
                }))
            }
            _ => panic!("unsupported transport protocol {}", protocol),
        };


        ctx.local().fds.insert(DescriptorId::new(fd), FdInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
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
            SocketState::Connectionless(_) => TransportAddress::Udp(socket_addr),
            SocketState::Unassociated(UnassociatedSocket { local_addr: Some(_), .. }) => {
                // Socket is already bound
                *libc::__errno_location() = libc::EINVAL;
                return -1
            },
            SocketState::Unassociated(UnassociatedSocket { protocol: TransportProtocol::Tcp, .. }) => TransportAddress::Tcp(socket_addr),
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

                match ctx.global().sockets.get_mut(socket_id).unwrap() {
                    SocketState::Unassociated(UnassociatedSocket { local_addr, .. }) => {
                        local_addr.replace(transport_addr);
                    }
                    SocketState::Connectionless(ConnectionlessSocket { local_addr, .. }) => {
                        // TODO: what if local_addr already had address? leak here...
                        *local_addr = socket_addr;
                    }
                    _ => panic!("internal state error in fizzle--unreachable code reached"),
                };


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
        let ready_to_connect = ctx.global().polled_events.put(PolledInfo::new());
        *ctx.global().sockets.get_mut(socket_id).unwrap() = SocketState::Server(ServerSocket {
            backend: IoBackend::Regular(()),
            local_addr,
            connecting: Queue::new(),
            ready_to_connect,
        });

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

                let server_backend = server_info.backend.clone();

                // Mark the socket as connected immediately, since it's connecting to a backend
                // NOTE: some programs may be confused by this--a connection immediately returning 0 is unusual for a transport protocol

                // TODO: write polled instances need to be raised by default in quite a few places...

                match ctx.global().sockets.get_mut(socket_id).unwrap() {
                    SocketState::Connecting(connecting_info) => {
                        let polled = connecting_info.connect_polled; // Delete the current polled instance TODO: fix to remove dangling refs
                        ctx.global().polled_events.remove(polled).unwrap();
                    },
                    _ => panic!("internal fizzle error"),
                }

                let connected_backend = match server_backend {
                    ServerBackend::Passthrough => unimplemented!(),
                    ServerBackend::Regular(()) => {
                        let SocketState::Server(server_info) = ctx.global().sockets.get_mut(server_socket_id).unwrap() else {
                            *libc::__errno_location() = libc::ECONNREFUSED;
                            return -1 // TODO: in the future, wait until a server does exist
                        };

                        // Don't actually create a connected backend--
                        let Ok(_) = server_info.connecting.enqueue(socket_id) else {
                            *libc::__errno_location() = libc::ECONNREFUSED;
                            return -1
                        };

                        let server_poll = server_info.ready_to_connect;
                        ctx.raise_polled(server_poll);

                        let client_poll = ctx.global().polled_events.put(PolledInfo::new());
                        *ctx.global().sockets.get_mut(socket_id).unwrap() = SocketState::Connecting(ConnectingSocket {
                            backend: ConnectingBackend::Regular(()),
                            connect_polled: client_poll,
                            local_addr,
                        });

                        return if is_nonblocking {
                            *libc::__errno_location() = libc::EINPROGRESS;
                            -1
                        } else {
                            ctx.poll_until_ready(client_poll); // TODO: if the server deletes this poll... UAF???
                            0
                        }
                    }
                    ServerBackend::Plugin(plugin_id) => {
                        // Create new plugin
                        let plugin_info = ctx.global().plugins.get(plugin_id).unwrap();
                        let endpoint = plugin_info.endpoint.clone();
                        let module_id = plugin_info.module_id;
                        let connect_plugin_id = ctx.global().add_plugin(endpoint, module_id);
                        ConnectedBackend::Plugin(connect_plugin_id)
                    },
                    ServerBackend::Sink => ConnectedBackend::Sink,
                    ServerBackend::Fuzz(_) => ConnectedBackend::Fuzz(0),
                    ServerBackend::NullSink => ConnectedBackend::NullSink,
                    ServerBackend::Feedback(()) => ConnectedBackend::Feedback(StandardFeedback {
                            buf: ctx.global().buffers.put(Buffer::new()),
                            read_polled: ctx.global().polled_events.put(PolledInfo::new()),
                            write_polled: ctx.global().polled_events.put(PolledInfo::new_raised()),
                    })
                };

                *ctx.global().sockets.get_mut(socket_id).unwrap() = SocketState::Connected(ConnectedSocket {
                    backend: connected_backend,
                    rem_addr,
                });

                0
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
            /*
            SocketState::Error => {
                *libc::__errno_location() = libc::ECONNREFUSED;
                -1
            }
            */
            SocketState::Connectionless(_) => panic!("invalid fizzle state--unexpected connectionless socket being connected to")
        }
    }
}

hook_macros::hook! {
    unsafe fn accept(
        fd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_accept(ctx) {
        drop(ctx);
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

            ctx.raise_polled(poll);

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
                crate::encode_inet_address(addr, connecting_info.local_addr.address());
            }

            let polled = connecting_info.connect_polled;
            ctx.raise_polled(polled);

            return join_socket_pair(&mut ctx, server_id, connecting_id, flags);

        } else if is_nonblocking {
            *libc::__errno_location() = libc::EAGAIN; // or EWOULDBLOCK
            return -1

        } else { // !is_nonblocking
            ctx.poll_until_ready(ready_to_connect);

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
                crate::encode_inet_address(addr, connecting_info.local_addr.address());
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
    let (server_addr, server_backend) = match ctx.global().sockets.get(server_id).unwrap() {
        SocketState::Server(server_info) => (server_info.local_addr, server_info.backend),
        _ => panic!("internal fizzle state"),
    };

    let accept_backend = match server_backend {
        IoBackend::Passthrough => unimplemented!(),
        IoBackend::Regular(_) => ConnectedBackend::Regular(RegularConnected {
            peer: Some(connecting_id),
            recv_buf: ctx.global().buffers.put(Buffer::new()),
            read_polled: ctx.global().polled_events.put(PolledInfo::new()),
            write_polled: ctx.global().polled_events.put(PolledInfo::new_raised()),
        }),
        _ => unreachable!(),
    };

    let (client_addr, connect_backend) = match ctx.global().sockets.get_mut(connecting_id).unwrap() {
        SocketState::PendingConnection(pending_info) => {
            let backend = pending_info.backend.clone();
            let protocol = pending_info.rem_addr.protocol();
            let family = pending_info.rem_addr.family();
            let local_addr = ctx.global().next_ephemeral_address(family, protocol);
            (local_addr, backend)
        }
        SocketState::Connecting(connecting_info) => {
            let client_addr = connecting_info.local_addr;
            let connect_backend = connecting_info.backend.clone();
            let polled = connecting_info.connect_polled; // Delete the current polled instance TODO: fix to remove dangling refs
            ctx.global().polled_events.remove(polled).unwrap();
            (client_addr, connect_backend)
        }
        _ => unreachable!(),
    };

    let connect_backend = match connect_backend {
        IoBackend::Passthrough => unimplemented!(),
        IoBackend::Regular(()) => ConnectedBackend::Regular(RegularConnected {
            peer: Some(connecting_id),
            recv_buf: ctx.global().buffers.put(Buffer::new()),
            read_polled: ctx.global().polled_events.put(PolledInfo::new()),
            write_polled: ctx.global().polled_events.put(PolledInfo::new_raised()),
        }),
        IoBackend::Feedback(()) => ConnectedBackend::Feedback(StandardFeedback {
            buf: ctx.global().buffers.put(Buffer::new()),
            read_polled: ctx.global().polled_events.put(PolledInfo::new()),
            write_polled: ctx.global().polled_events.put(PolledInfo::new_raised()),
        }),
        IoBackend::Plugin(plugin_id) => ConnectedBackend::Plugin(plugin_id),
        IoBackend::Sink => ConnectedBackend::Sink,
        IoBackend::NullSink => ConnectedBackend::NullSink,
        IoBackend::Fuzz(_) => ConnectedBackend::Fuzz(0),
    };

    let accepted_id = ctx
        .global()
        .sockets
        .put(SocketState::Connected(ConnectedSocket {
            rem_addr: client_addr,
            backend: accept_backend,
        }));
    
    *ctx.global().sockets.get_mut(connecting_id).unwrap() =
        SocketState::Connected(ConnectedSocket {
            rem_addr: server_addr,
            backend: connect_backend,
        });

    // The two sockets are now joined--add a file descriptor to the accepted socket
    ctx.local().fds.insert(
        DescriptorId::new(crate::alias_fd_create()),
        FdInfo {
            close_on_exec: (flags & libc::O_CLOEXEC) != 0,
            is_passthrough: false,
            nonblocking: (flags & libc::O_NONBLOCK) != 0,
            resource: FdResource::Socket(accepted_id),
        },
    );

    0 // TODO: need to account for error conditions within this function
}

// TODO: UDP sockets bound addresses (yes, even ephemeral) need to be registered


const SOL_SCTP: i32 = 132;

hook_macros::hook! {
    unsafe fn getsockopt(
        sockfd: libc::c_int,
        level: libc::c_int,
        optname: libc::c_int,
        optval: *mut libc::c_void,
        optlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_getsockopt(ctx) {

        let descriptor_id = DescriptorId::new(sockfd);
        let Some(fd_info) = ctx.local().fds.get(descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match (level, optname) {
            (libc::SOL_SOCKET, libc::SO_ACCEPTCONN) => {
                let is_listening = match ctx.global().sockets.get(socket_id).unwrap() {
                    SocketState::Server(_) => 1,
                    _ => 0
                };

                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                *(optval as *mut libc::c_int) = is_listening;
                0
            }
            (libc::SOL_SOCKET, libc::SO_ATTACH_FILTER | libc::SO_LOCK_FILTER | libc::SO_ATTACH_BPF | libc::SO_ATTACH_REUSEPORT_CBPF | libc::SO_ATTACH_REUSEPORT_EBPF) => {
                crate::report_strict_failure("unsupported BPF `getsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (libc::SOL_SOCKET, libc::SO_BINDTODEVICE) => {
                crate::report_strict_failure("unsupported SO_BINDTODEVICE `getsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (libc::SOL_SOCKET, libc::SO_BROADCAST) => {
                crate::report_strict_failure("unsupported SO_BROADCAST `getsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            // SO_DEBUG, SO_DETACH_FILTER, SO_DONTROUTE, SO_INCOMING_CPU, SO_INCOMING_NAPI_ID
            (libc::SOL_SOCKET, libc::SO_DOMAIN) => {
                let domain = match ctx.global().sockets.get(socket_id).unwrap() {
                    SocketState::Connectionless(sock_info) => match sock_info.local_addr {
                        SocketAddr::V4(_) => libc::AF_INET,
                        SocketAddr::V6(_) => libc::AF_INET6,
                    },
                    SocketState::Unassociated(sock_info) => match sock_info.family {
                        AddressFamily::Ipv4 => libc::AF_INET,
                        AddressFamily::Ipv6 => libc::AF_INET6,
                    },
                    SocketState::Server(server_info) => match server_info.local_addr.address() {
                        SocketAddr::V4(_) => libc::AF_INET,
                        SocketAddr::V6(_) => libc::AF_INET6,
                    },
                    SocketState::PendingConnection(pending_info) => match pending_info.rem_addr.address() {
                        SocketAddr::V4(_) => libc::AF_INET,
                        SocketAddr::V6(_) => libc::AF_INET6,
                    },
                    SocketState::Connecting(connecting_info) => match connecting_info.local_addr.address() {
                        SocketAddr::V4(_) => libc::AF_INET,
                        SocketAddr::V6(_) => libc::AF_INET6,
                    },
                    SocketState::Connected(connected_info) => match connected_info.rem_addr.address() {
                        SocketAddr::V4(_) => libc::AF_INET,
                        SocketAddr::V6(_) => libc::AF_INET6,
                    },
                };

                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                *(optval as *mut libc::c_int) = domain;
                0
            }
            (libc::SOL_SOCKET, libc::SO_ERROR) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // TODO: update this value if legitimate errors ever occur during polling.
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (libc::SOL_SOCKET, libc::SO_KEEPALIVE) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Pretend keepalive is enabled
                *(optval as *mut libc::c_int) = 1;
                0
            }
            (libc::SOL_SOCKET, libc::SO_LINGER) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::linger>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Pretend linger is disabled
                *(optval as *mut libc::linger) = libc::linger { l_onoff: 0, l_linger: 0 };
                0
            }
            // SO_MARK
            (libc::SOL_SOCKET, libc::SO_OOBINLINE) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Pretend in-line OOB is enabled
                *(optval as *mut libc::c_int) = 1;
                0
            }
            // SO_PASSCRED, SO_PASSSEC, SO_PEEK_OFF, SO_PEERCRED, SO_PEERSEC
            (libc::SOL_SOCKET, libc::SO_PRIORITY) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Pretend the priority of all sockets is always 6
                *(optval as *mut libc::c_int) = 6;
                0
            }
            (libc::SOL_SOCKET, libc::SO_PROTOCOL) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                let protocol = match ctx.global().sockets.get(socket_id).unwrap() {
                    SocketState::Connectionless(_) => libc::IPPROTO_UDP,
                    SocketState::Unassociated(unassociated_info) => match unassociated_info.protocol {
                        TransportProtocol::Tcp => libc::IPPROTO_TCP,
                        TransportProtocol::Udp => libc::IPPROTO_UDP,
                        TransportProtocol::Sctp => libc::IPPROTO_SCTP,
                    },
                    SocketState::Server(server_info) => match server_info.local_addr {
                        TransportAddress::Tcp(_) => libc::IPPROTO_TCP,
                        TransportAddress::Udp(_) => libc::IPPROTO_UDP,
                        TransportAddress::Sctp(_) => libc::IPPROTO_SCTP,
                    },
                    SocketState::PendingConnection(pending_info) => match pending_info.rem_addr {
                        TransportAddress::Tcp(_) => libc::IPPROTO_TCP,
                        TransportAddress::Udp(_) => libc::IPPROTO_UDP,
                        TransportAddress::Sctp(_) => libc::IPPROTO_SCTP,
                    },
                    SocketState::Connecting(connecting_info) => match connecting_info.local_addr {
                        TransportAddress::Tcp(_) => libc::IPPROTO_TCP,
                        TransportAddress::Udp(_) => libc::IPPROTO_UDP,
                        TransportAddress::Sctp(_) => libc::IPPROTO_SCTP,
                    },
                    SocketState::Connected(connected_info) => match connected_info.rem_addr {
                        TransportAddress::Tcp(_) => libc::IPPROTO_TCP,
                        TransportAddress::Udp(_) => libc::IPPROTO_UDP,
                        TransportAddress::Sctp(_) => libc::IPPROTO_SCTP,
                    },
                };

                // Pretend the priority of all sockets is always 6
                *(optval as *mut libc::c_int) = protocol;
                0
            }
            (libc::SOL_SOCKET, libc::SO_RCVBUF) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Buffers are fixed size, always.
                *(optval as *mut libc::c_int) = FIZZLE_BUFFER_LENGTH as libc::c_int;
                0
            }
            (libc::SOL_SOCKET, libc::SO_SNDLOWAT | libc::SO_RCVLOWAT) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Buffers are immediately received once one byte of data has been written.
                *(optval as *mut libc::c_int) = 1;
                0
            }
            (libc::SOL_SOCKET, libc::SO_RCVTIMEO | libc::SO_SNDTIMEO) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::timeval>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Never any timeouts among sockets
                *(optval as *mut libc::timeval) = libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                };
                0
            }
            (libc::SOL_SOCKET, libc::SO_REUSEADDR) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Never any timeouts among sockets
                *(optval as *mut libc::c_int) = 1;
                0
            }
            (libc::SOL_SOCKET, libc::SO_REUSEPORT) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Never any timeouts among sockets
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (libc::SOL_SOCKET, _) => panic!("unrecognized getsockopt SOL_SOCKET, optname {}", optname),
            // TODO: implement SO_RXQ_OVFL, SO_TIMESTAMP, when implementing `cmsg`s
            (SOL_SCTP, libc::SCTP_RTOINFO) => {
                // libc::sctp_rtoinfo not defined...

                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_ASSOCINFO) => {
                // libc::sctp_assocparams not defined...

                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_INITMSG) => {
                // libc::sctp_initmsg not defined...

                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_NODELAY) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // NODELAY always enabled
                *(optval as *mut libc::c_int) = 1;
                0
            }
            (SOL_SCTP, libc::SCTP_AUTOCLOSE) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // autoclose always disabled
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (SOL_SCTP, libc::SCTP_SET_PEER_PRIMARY_ADDR) => {
                // Set option only...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_PRIMARY_ADDR) => {
                // libc::sctp_prim not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_DISABLE_FRAGMENTS) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // fragments always enabled
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (SOL_SCTP, libc::SCTP_PEER_ADDR_PARAMS) => {
                // libc::sctp_paddrparams not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_DEFAULT_SEND_PARAM) => {
                // libc::sctp_sndrcvinfo not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_EVENTS) => {
                // libc::sctp_event_subscribe not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_I_WANT_MAPPED_V4_ADDR) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Mapped IPv4 always disabled
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (SOL_SCTP, libc::SCTP_MAXSEG) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Fragmentation not limited
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (SOL_SCTP, libc::SCTP_STATUS) => {
                // libc::sctp_status not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_GET_PEER_ADDR_INFO) => {
                // libc::sctp_paddrinfo not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, 112) => { // libc::SCTP_GET_ASSOC_STATS
                // libc::sctp_assoc_stats not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, _) => {
                panic!("Unrecognized socket option: SOL_SCTP, optname {}", optname);
            }
            _ => {
                panic!("Unrecognized socket option: level {}, optname {}", level, optname);
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn setsockopt(
        sockfd: libc::c_int,
        level: libc::c_int,
        optname: libc::c_int,
        _optval: *const libc::c_void,
        optlen: libc::socklen_t
    ) -> libc::c_int => fizzle_setsockopt(ctx) {

        let descriptor_id = DescriptorId::new(sockfd);
        let Some(fd_info) = ctx.local().fds.get(descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(_socket_id) = fd_info.resource else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match (level, optname) {
            // Socket options that are readonly
            (libc::SOL_SOCKET, libc::SO_ACCEPTCONN | libc::SO_DOMAIN | libc::SO_ERROR | libc::SO_PROTOCOL) => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            }
            // Socket options that we pretend to support (but don't)
            (libc::SOL_SOCKET, libc::SO_KEEPALIVE | libc::SO_OOBINLINE | libc::SO_PRIORITY | libc::SO_RCVBUF | libc::SO_SNDLOWAT | libc::SO_RCVLOWAT | libc::SO_RCVTIMEO | libc::SO_SNDTIMEO | libc::SO_REUSEADDR | libc::SO_REUSEPORT) => {
                // TODO: is libc this strict, or not?
                if optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL; 
                    return -1
                }

                // Ignore received value
                0
            }
            (libc::SOL_SOCKET, libc::SO_ATTACH_FILTER | libc::SO_LOCK_FILTER | libc::SO_ATTACH_BPF | libc::SO_ATTACH_REUSEPORT_CBPF | libc::SO_ATTACH_REUSEPORT_EBPF) => {
                crate::report_strict_failure("unsupported BPF `getsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (libc::SOL_SOCKET, libc::SO_BINDTODEVICE) => {
                crate::report_strict_failure("unsupported SO_BINDTODEVICE `getsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (libc::SOL_SOCKET, libc::SO_BROADCAST) => {
                crate::report_strict_failure("unsupported SO_BROADCAST `getsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }

            (libc::SOL_SOCKET, libc::SO_LINGER) => {
                // TODO: is libc this strict, or not?
                if optlen as usize != mem::size_of::<libc::linger>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Pretend linger is disabled
                0
            }
            // TODO: implement SO_RXQ_OVFL, SO_TIMESTAMP, when implementing `cmsg`s
            (libc::SOL_SOCKET, _) => {
                panic!("Unrecognized socket option: SOL_SOCKET, optname {}", optname);
            }
            (SOL_SCTP, libc::SCTP_RTOINFO | libc::SCTP_ASSOCINFO | libc::SCTP_INITMSG | libc::SCTP_NODELAY | libc::SCTP_AUTOCLOSE | libc::SCTP_DISABLE_FRAGMENTS | libc::SCTP_PEER_ADDR_PARAMS | libc::SCTP_DEFAULT_SEND_PARAM | libc::SCTP_EVENTS | libc::SCTP_MAXSEG) => {
                // Ignore received value
                0
            }
            (SOL_SCTP, libc::SCTP_SET_PEER_PRIMARY_ADDR | libc::SCTP_PRIMARY_ADDR | libc::SCTP_I_WANT_MAPPED_V4_ADDR) => {
                // Ignoring received value would cause issues
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_STATUS | libc::SCTP_GET_PEER_ADDR_INFO | 112) => { // libc::SCTP_GET_ASSOC_STATS
                // readonly option
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, _) => {
                panic!("Unrecognized socket option: SOL_SCTP, optname {}", optname);
            }
            _ => {
                panic!("Unrecognized socket option: level {}, optname {}", level, optname);
            }
        }
    }
}
