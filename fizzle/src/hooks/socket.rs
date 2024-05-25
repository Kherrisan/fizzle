//! Process I/O shims.
//!
//!

use std::{array, mem};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::hook_macros;
use crate::state::fd::{FdInfo, FdResource};
use crate::state::identifiers::DescriptorId;
use crate::state::{AddressFamily, ClientInfo, ClientState, IoBackend, SocketInfo, SocketLocationInfo, SocketVariant};
use fizzle_common::io::{TransportAddress, TransportProtocol};

unsafe fn parse_internet_address(protocol: TransportProtocol, addr: *const libc::sockaddr, addrlen: libc::socklen_t) -> TransportAddress {
    match (*addr).sa_family as i32 {
        libc::AF_INET => {
            let addr = addr as *const libc::sockaddr_in;
            if addrlen as usize != mem::size_of::<libc::sockaddr_in>() {
                panic!("invalid socket length for received sockaddr_in")
                // *libc::__errno_location() = libc::EINVAL;
                // return -1
            }

            // TODO: verify correctness of these conversions
            let addr_bytes = u32::from_be((*addr).sin_addr.s_addr).to_be_bytes();
            let port = u16::from_be((*addr).sin_port);
            TransportAddress {
                protocol: protocol,
                sockaddr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(addr_bytes[0], addr_bytes[1], addr_bytes[2], addr_bytes[3]), port)),
            }
        }
        libc::AF_INET6 => {
                let addr = addr as *const libc::sockaddr_in6;
            if addrlen as usize != mem::size_of::<libc::sockaddr_in6>() {
                panic!("invalid socket length for received sockaddr_in6")
                //*libc::__errno_location() = libc::EINVAL;
                //return -1
            }

            // TODO: verify correctness of these conversions
            let addr_segments: [u16; 8] = array::from_fn(|i| u16::from_be_bytes((*addr).sin6_addr.s6_addr[2*i..(2*i) + 2].try_into().unwrap())); // TODO: replace with newer libc functions when they arrive
            let port = u16::from_be((*addr).sin6_port);
            let flow_info = u32::from_be((*addr).sin6_flowinfo);
            let scope_id = u32::from_be((*addr).sin6_scope_id);
            TransportAddress {
                protocol: protocol,
                sockaddr: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::new(addr_segments[0], addr_segments[1], addr_segments[2], addr_segments[3], addr_segments[4], addr_segments[5], addr_segments[6], addr_segments[7]), port, flow_info, scope_id)),
            }
        }
        _ => panic!("fizzle does not currently support address family {}", (*addr).sa_family),
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

        let socket_id = ctx.global().sockets.put(SocketInfo {
            variant: SocketVariant::Unassociated,
            protocol,
            family,
            local_addr: None,
            rem_addr: None, // TODO: check to ensure these are correct
        });

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

        let Some(sock_info) = ctx.global().sockets.get(socket_id) else {
            panic!("invalid internal fizzle state--sock_info not found during `bind()`")
        };

        let transport_addr = parse_internet_address(sock_info.protocol, addr, addrlen);

        match ctx.global().socket_locations.entry(transport_addr) {
            heapless::Entry::Occupied(_) => {
                *libc::__errno_location() = libc::EADDRINUSE;
                -1
            },
            heapless::Entry::Vacant(v) => {
                v.insert(SocketLocationInfo {
                    bound_socket: Some(socket_id),
                    pending_client: None,
                }).unwrap();

                0
            },
        }
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

        let Some(sock_info) = ctx.global().sockets.get_mut(socket_id) else {
            panic!("invalid internal fizzle state--sock_info not found during `bind()`")
        };

        // TODO: support AF_UNSPEC?
        let transport_addr = parse_internet_address(sock_info.protocol, addr, addrlen);

        if transport_addr.protocol == TransportProtocol::Udp {
            // Connectionless protocol--just remember the address for future `send()`/`recv()` calls...
            // TODO: include UNIX Datagram sockets here in the future...
            sock_info.rem_addr = Some(transport_addr);
            return 0
        }

        // Connection-oriented protocol--handle connection state
        match sock_info.variant {
            SocketVariant::Unassociated => {
                // First: is the address currently bound?
                match ctx.global().socket_locations.get(&transport_addr) {
                    Some(SocketLocationInfo { bound_socket: Some(socket_id), .. }) => {
                        // There is a socket
                        let socket_id = *socket_id;
                        let socket_info = ctx.global().sockets.get(socket_id).unwrap();
                        
                        // Second: is the socket bound to the address listening for connections?
                        match socket_info.variant {
                            // Bound socket for the given address was not listening...
                            SocketVariant::Unassociated | SocketVariant::Connected(_) => {

                                // TODO: maybe wait until a server does exist at the location?
                                *libc::__errno_location() = libc::ECONNREFUSED;
                                return -1
                            }
                            // Bound socket for the given address is listening
                            SocketVariant::Server(server_id) => {
                                let server_info = ctx.global().servers.get_mut(server_id).unwrap();
                                todo!()
                                /*
                                if let Some(worker) = server_info.awaiting_connection.take() {
                                    // push worker onto queue
                                }// else if let IoBackend::
                                //server_info
                                */
                            }
                            // Bound socket for the given address is not capable of listening??
                            SocketVariant::Client(_) => panic!("fizzle internal state error--client socket where server should be"),
                        }

                        //let server_info = ctx.global().servers.get(server_id).unwrap();
                    },
                    _ => {
                        // No bound socket was found for the given address...
                        // TODO: maybe wait until a server does exist at the location??
                        *libc::__errno_location() = libc::ECONNREFUSED;
                        return -1;
                    },
                }
                if ctx.global().socket_locations.contains_key(&transport_addr) {
                    
                }
                todo!()
                /*
                let client_id = ctx.global().clients.put(ClientInfo {
                    awaiting_connection: None,
                    backend,
                    state: ClientState::PendingConnection,
                });
                sock_info.variant = SocketVariant::Client(client_id);
                */
            },
            SocketVariant::Client(client_id) => todo!(),
            SocketVariant::Server(_) => panic!("program attempted to `connect()` a listening server socket (fizzle internal error)"),
            SocketVariant::Connected(_) => {
                *libc::__errno_location() = libc::EISCONN;
                return -1
            },
        }

        if is_nonblocking {
            *libc::__errno_location() = libc::EWOULDBLOCK;
            -1
        } else {
            0
        }
    }
}
