//! Process I/O shims.
//!
//!

use std::mem::MaybeUninit;
use std::{mem, ptr, slice};

use crate::backend::{ConnectionlessBackend, RegularConnectionless};
use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::handlers::descriptor::{DescriptorId, DescriptorInfo, FdResource};
use crate::handlers::polled::PolledInfo;
use crate::handlers::socket::{ConnectionlessSocket, SocketState, UnassociatedSocket};
use crate::handlers::FfiOutput;
use crate::hook_macros;

use fizzle_common::io::{AddressFamily, SockAddr, SocketType, TransportProtocol};
use fizzle_common::storage::Buffer;

hook_macros::hook! {
    unsafe fn socket(
        domain: libc::c_int,
        socktype: libc::c_int,
        protocol: libc::c_int
    ) -> libc::c_int => fizzle_socket(ctx) {
        let mut state = ctx.acquire();

        let nonblocking = (socktype & libc::SOCK_NONBLOCK) != 0;
        let close_on_exec = (socktype & libc::SOCK_CLOEXEC) != 0;
        let socktype = socktype & !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC);

        let family = match domain {
            libc::AF_INET => AddressFamily::Ipv4,
            libc::AF_INET6 => AddressFamily::Ipv6,
            libc::AF_UNIX => AddressFamily::Unix,
            _ => panic!("unsupported socket domain {}", domain),
        };

        let socket_type = match socktype {
            libc::SOCK_STREAM => SocketType::Stream,
            libc::SOCK_SEQPACKET => SocketType::SeqPacket,
            libc::SOCK_DGRAM => SocketType::Datagram,
            _ => panic!("unsupported socket type {}", socktype),
        };

        let transport_protocol = match (family, protocol) {
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, 0 | libc::IPPROTO_TCP) => TransportProtocol::Tcp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, libc::IPPROTO_UDP) => TransportProtocol::Udp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, libc::IPPROTO_SCTP) => TransportProtocol::Sctp,
            (AddressFamily::Unix, 0) => TransportProtocol::Unix,
            _ => panic!("unrecognized socket family/protocol pair {}, {}", family, protocol),
        };

        let fd = hook_macros::real!(socket)(domain, socktype, protocol);

        if fd < 0 {
            crate::strace!("socket(domain={}, socktype={}, protocol={}) -> -1 ({})", family, socket_type, transport_protocol, crate::errno_str());
            return fd
        }

        let socket_id = match socket_type {
            SocketType::SeqPacket | SocketType::Stream => state.global.sockets.allocate(SocketState::Unassociated(
                UnassociatedSocket::new(family, socket_type, transport_protocol))
            ).unwrap(),
            SocketType::Datagram => {
                let local_addr = state.global.ephemeral_address(family, transport_protocol);
                let recv_buf = state.global.buffers.allocate(Buffer::new()).unwrap();
                let read_polled = state.global.polled_events.allocate(PolledInfo::new()).unwrap();
                let write_polled = state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap();

                state.global.sockets.allocate(SocketState::Connectionless(ConnectionlessSocket {
                    backend: ConnectionlessBackend::Peered(RegularConnectionless {
                        recv_buf,
                        read_polled,
                        write_polled,
                    }),
                    local_addr,
                    rem_addr: None,
                    reuse_port: false,
                })).unwrap()
            }
        };

        let descriptor_id = DescriptorId::from_raw_fd(fd);
        state.local.fds.allocate_with_key(descriptor_id, DescriptorInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Socket(socket_id)
        }).unwrap();

        crate::strace!("socket(domain={}, socktype={}, protocol={}) -> 0", family, socket_type, transport_protocol);

        fd
    }
}

hook_macros::hook! {
    unsafe fn socketpair(
        _domain: libc::c_int,
        _ty: libc::c_int,
        _protocol: libc::c_int,
        _sv: *mut [libc::c_int; 2]
    ) => fizzle_clearerr(_ctx) {
        unimplemented!("socketpair()")
    }
}

hook_macros::hook! {
    unsafe fn bind(
        fd: libc::c_int,
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::c_int => fizzle_bind(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
        let addr_bytes = slice::from_raw_parts(addr as *const u8, addrlen as usize);

        let Ok(addr) = SockAddr::decode(addr_bytes) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        drop(state);

        let res = socket_id.bind(&mut ctx, addr.clone());
        crate::strace!("bind(fd={}, addr={}) -> {}", fd, &addr, res.display());
        res.out()
    }
}

hook_macros::hook! {
    unsafe fn listen(
        fd: libc::c_int,
        backlog: libc::c_int
    ) -> libc::c_int => fizzle_listen(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        drop(state);

        let res = socket_id.listen(&mut ctx);
        crate::strace!("listen(fd={}, backlog={}) -> {}", fd, backlog, res.display());
        res.out()
    }
}

hook_macros::hook! {
    unsafe fn connect(
        fd: libc::c_int,
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::c_int => fizzle_connect(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let nonblocking = fd_info.nonblocking;

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
        let addr_bytes = slice::from_raw_parts(addr as *const u8, addrlen as usize);

        let Ok(rem_addr) = SockAddr::decode(addr_bytes) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        drop(state);

        if nonblocking {
            crate::strace!("connect(fd={}, rem_addr={}) -> ...", fd, rem_addr);
        }

        let res = socket_id.connect(&mut ctx, rem_addr.clone(), nonblocking);
        crate::strace!("connect(fd={}, rem_addr={}) -> {}", fd, rem_addr, res.display());
        res.out()
    }
}

hook_macros::hook! {
    unsafe fn accept(
        fd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_accept(_ctx) {
        crate::strace!("accept -> accept4");
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
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let nonblocking = fd_info.nonblocking | (flags & libc::O_NONBLOCK > 0);
        let close_on_exec = flags & libc::O_CLOEXEC > 0;

        let FdResource::Socket(server_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        drop(state);
        let res = server_id.accept(&mut ctx, nonblocking, close_on_exec);
        crate::strace!("accept4(fd={}, flags={}) -> {}", fd, match (nonblocking, close_on_exec) {
            (false, false) => "0",
            (false, true) => "O_CLOEXEC",
            (true, false) => "O_NONBLOCK",
            (true, true) => "O_NONBLOCK | O_CLOEXEC",
        }, res.display());

        if !addr.is_null() && !addrlen.is_null() {
            if let Ok((_, conn_addr)) = res.as_ref() {
                log::info!("accepted connection from address {}", conn_addr);
                let addr_bytes = slice::from_raw_parts_mut(addr as *mut MaybeUninit<u8>, *addrlen as usize);
                *addrlen = conn_addr.encode(addr_bytes) as libc::socklen_t;
            }
        }

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn getsockname(
        sockfd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_getsockname(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(sockfd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        drop(state);

        let res = socket_id.socket_name(&mut ctx);
        crate::strace!("getsockname(fd={}) -> {}", sockfd, res.display());

        if let Ok(sockname) = res.as_ref() {
            log::debug!("getsockname() mapped fd {} to address {}", sockfd, sockname);
            let addr_bytes = slice::from_raw_parts_mut(addr as *mut MaybeUninit<u8>, *addrlen as usize);
            *addrlen = sockname.encode(addr_bytes) as libc::socklen_t;
        }

        res.out()
    }
}

#[repr(C)]
struct SctpRtoInfo {
    srto_assoc_id: libc::sctp_assoc_t,
    srto_initial: u32,
    srto_max: u32,
    srto_min: u32,
}

#[repr(C)]
struct SctpGetaddrs {
    assoc_id: libc::sctp_assoc_t, // input
    addr_num: i32,                // output
    addrs: *mut u8,               // output, variable size
}

#[allow(non_camel_case_types, unused)]
#[repr(packed)]
struct sctp_paddrparams {
    spp_assoc_id: libc::sctp_assoc_t,
    spp_address: libc::sockaddr_storage,
    spp_hbinterval: u32,
    spp_pathmaxrxt: u16,
    spp_pathmtu: u32,
    spp_sackdelay: u32,
    spp_flags: u32,
    spp_ipv6_flowlabel: u32,
    spp_dscp: u8,
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct sctp_assocparams {
    sasoc_assoc_id: libc::sctp_assoc_t,
    sasoc_asocmaxrxt: u16,
    sasoc_peer_rwnd: u32,
    sasoc_local_rwnd: u32,
    sasoc_cookie_life: u32,
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct sctp_initmsg {
    sinit_num_ostreams: u16,
    sinit_max_instreams: u16,
    sinit_max_attempts: u16,
    sinit_max_init_timeo: u16,
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct sctp_event_subscribe {
    sctp_data_io_event: u8,
    sctp_association_event: u8,
    sctp_address_event: u8,
    sctp_send_failure_event: u8,
    sctp_peer_error_event: u8,
    sctp_shutdown_event: u8,
    sctp_partial_delivery_event: u8,
    sctp_adaptation_layer_event: u8,
    sctp_authentication_event: u8,
    sctp_sender_dry_event: u8,
    sctp_stream_reset_event: u8,
    sctp_assoc_reset_event: u8,
    sctp_stream_change_event: u8,
    sctp_send_failure_event_event: u8,
}

const SOL_SCTP: i32 = 132;
const SCTP_SOCKOPT_BINDX_ADD: i32 = 100;
const SCTP_SOCKOPT_BINDX_REM: i32 = 101;
// const SCTP_SOCKOPT_PEELOFF: i32 = 102;

const SCTP_SOCKOPT_CONNECTX_OLD: i32 = 107;
const SCTP_GET_PEER_ADDRS: i32 = 108;
const SCTP_GET_LOCAL_ADDRS: i32 = 109;
const SCTP_SOCKOPT_CONNECTX: i32 = 110;
const SCTP_SOCKOPT_CONNECTX3: i32 = 111;
const SCTP_GET_ASSOC_STATS: i32 = 112;
const SCTP_PR_SUPPORTED: i32 = 113;

hook_macros::hook! {
    unsafe fn getsockopt(
        sockfd: libc::c_int,
        level: libc::c_int,
        optname: libc::c_int,
        optval: *mut libc::c_void,
        optlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_getsockopt(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(sockfd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match (level, optname) {
            (libc::SOL_IP, libc::IP_OPTIONS) => {
                *optlen = 0;
                0
            }
            (libc::SOL_IP, _) => {
                log::error!("Unrecognized socket option: SOL_IP, optname {}", optname);
                panic!("Unrecognized socket option: SOL_IP, optname {}", optname);
            }
            (libc::SOL_TCP, libc::TCP_USER_TIMEOUT) => {
                *(optval as *mut libc::c_int) = 20000;
                0
            }
            (libc::SOL_TCP, libc::TCP_NODELAY) => {
                *(optval as *mut libc::c_int) = 1;
                0
            }
            (libc::SOL_TCP, libc::TCP_MAXSEG) => {
                *(optval as *mut libc::c_int) = 1220;
                0
            }
            (libc::SOL_TCP, _) => panic!("unrecognized getsockopt SOL_TCP option {}", optname),
            (libc::SOL_SOCKET, libc::SO_ACCEPTCONN) => {
                let is_listening = match state.global.sockets.get(&socket_id).unwrap() {
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
                let domain = match state.global.sockets.get(&socket_id).unwrap() {
                    SocketState::Connectionless(sock_info) => sock_info.local_addr.family().raw(),
                    SocketState::Unassociated(sock_info) => sock_info.family.raw(),
                    SocketState::Server(server_info) => server_info.local_addr.family().raw(),
                    SocketState::PendingConnection(pending_info) => pending_info.rem_addr.family().raw(),
                    SocketState::Connecting(connecting_info) => connecting_info.local_addr.family().raw(),
                    SocketState::Connected(connected_info) => connected_info.rem_addr.family().raw(),
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
            (libc::SOL_SOCKET, libc::SO_ZEROCOPY) => {
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Pretend zero-copy is enabled
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

                let protocol = match state.global.sockets.get(&socket_id).unwrap() {
                    SocketState::Connectionless(_) => libc::IPPROTO_UDP,
                    SocketState::Unassociated(unassociated_info) => unassociated_info.protocol.raw(),
                    SocketState::Server(server_info) => server_info.local_addr.protocol().raw(),
                    SocketState::PendingConnection(pending_info) => pending_info.rem_addr.protocol().raw(),
                    SocketState::Connecting(connecting_info) => connecting_info.local_addr.protocol().raw(),
                    SocketState::Connected(connected_info) => connected_info.rem_addr.protocol().raw(),
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

                *(optval as *mut libc::c_int) = 1;
                0
            }
            (libc::SOL_SOCKET, _) => panic!("unrecognized getsockopt SOL_SOCKET, optname {}", optname),
            // TODO: implement SO_RXQ_OVFL, SO_TIMESTAMP, when implementing `cmsg`s
            (SOL_SCTP, libc::SCTP_RTOINFO) => {
                // libc::sctp_rtoinfo not defined...
                // TODO: is libc this strict, or not?
                if *optlen as usize != mem::size_of::<SctpRtoInfo>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Never any timeouts among sockets
                *(optval as *mut SctpRtoInfo) = SctpRtoInfo {
                    srto_assoc_id: 0,
                    srto_initial: 3000,
                    srto_max: 60000,
                    srto_min: 1000,
                }; // based on default values for Debian 12/Linux 6.XX

                0
            }
            (SOL_SCTP, SCTP_GET_LOCAL_ADDRS) => {

                let assoc_id = (*(optval as *const SctpGetaddrs)).assoc_id;
                *(optval as *mut SctpGetaddrs) = SctpGetaddrs { assoc_id, addr_num: 0, addrs: ptr::null_mut() };

                0
            }
            (SOL_SCTP, libc::SCTP_INITMSG) => {

                *(optval as *mut sctp_initmsg) = sctp_initmsg {
                    sinit_num_ostreams: 10,
                    sinit_max_instreams: 10,
                    sinit_max_attempts: 8,
                    sinit_max_init_timeo: 60000
                };

                0
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

                let spp_assoc_id = (*(optval as *mut sctp_paddrparams)).spp_assoc_id;
                let spp_address = (*(optval as *mut sctp_paddrparams)).spp_address;

                *(optval as *mut sctp_paddrparams) = sctp_paddrparams {
                    spp_assoc_id,
                    spp_address,
                    spp_hbinterval: 30000,
                    spp_pathmaxrxt: 5,
                    spp_pathmtu: 1260,
                    spp_sackdelay: 200,
                    spp_flags: 1 | (1 << 3) | (1 << 5),
                    spp_ipv6_flowlabel: 0,
                    spp_dscp: 0
                };
                0
            }
            (SOL_SCTP, libc::SCTP_DEFAULT_SEND_PARAM) => {
                // libc::sctp_sndrcvinfo not defined...
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_EVENTS) => {
                // libc::sctp_event_subscribe not defined...
                *(optval as *mut sctp_event_subscribe) = sctp_event_subscribe {
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
                };

                0
            }
            (SOL_SCTP, libc::SCTP_I_WANT_MAPPED_V4_ADDR) => {
                // Mapped IPv4 always disabled
                *(optval as *mut libc::c_int) = 0;
                0
            }
            (SOL_SCTP, libc::SCTP_FRAGMENT_INTERLEAVE) => {
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
            (SOL_SCTP, libc::SCTP_ASSOCINFO) => {
                let sasoc_assoc_id = (*(optval as *mut sctp_assocparams)).sasoc_assoc_id;
                (*(optval as *mut sctp_assocparams)) = sctp_assocparams {
                    sasoc_assoc_id,
                    sasoc_asocmaxrxt: 10,
                    sasoc_peer_rwnd: 1,
                    sasoc_local_rwnd: 1,
                    sasoc_cookie_life: 60000
                };
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
            (SOL_SCTP, _) => panic!("Unrecognized socket option: SOL_SCTP, optname {}", optname),
            (libc::SOL_IPV6, libc::IPV6_V6ONLY) => {
                *(optval as *mut libc::c_int) = 1;
                0 // Pretend to have V6ONLY enabled
            }
            (libc::SOL_IPV6, _) => panic!("Unrecognized socket option: SOL_IPV6, optname {}", optname),
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
        optval: *const libc::c_void,
        optlen: libc::socklen_t
    ) -> libc::c_int => fizzle_setsockopt(ctx) {
        let state = ctx.acquire();

        log::info!("setsockopt({}, {}, {}, {:?}, {} (value {}))", sockfd, level, optname, optval, optlen, *(optval as *const libc::c_int));

        let descriptor_id = DescriptorId::from_raw_fd(sockfd);
        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            *libc::__errno_location() = libc::EBADF;
            return -1
        };

        let FdResource::Socket(_socket_id) = fd_info.resource.clone() else {
            *libc::__errno_location() = libc::ENOTSOCK;
            return -1
        };

        match (level, optname) {
            (libc::SOL_IP, libc::IP_OPTIONS) => {
                0
            }
            (libc::SOL_IP, _) => {
                log::error!("Unrecognized socket option: SOL_IP, optname {}", optname);
                panic!("Unrecognized socket option: SOL_IP, optname {}", optname);
            }
            // Pretend to support (but don't)
            (libc::SOL_TCP, libc::TCP_NODELAY | libc::TCP_MAXSEG | libc::TCP_USER_TIMEOUT | libc::TCP_FASTOPEN) => {
                0
            }
            (libc::SOL_TCP, _) => {
                panic!("Unrecognized socket option: SOL_TCP, optname {}", optname);
            }
            // Socket options that are readonly
            (libc::SOL_SOCKET, libc::SO_ACCEPTCONN | libc::SO_DOMAIN | libc::SO_ERROR | libc::SO_PROTOCOL) => {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            }
            // Socket options that we pretend to support (but don't)
            (libc::SOL_SOCKET, libc::SO_KEEPALIVE | libc::SO_OOBINLINE | libc::SO_PRIORITY | libc::SO_RCVBUF | libc::SO_SNDLOWAT | libc::SO_RCVLOWAT | libc::SO_RCVTIMEO | libc::SO_SNDTIMEO | libc::SO_REUSEADDR | libc::SO_REUSEPORT | libc::SO_ZEROCOPY) => {
                // TODO: is libc this strict, or not?
                if optlen as usize != mem::size_of::<libc::c_int>() {
                    *libc::__errno_location() = libc::EINVAL;
                    return -1
                }

                // Ignore received value
                0
            }
            (libc::SOL_SOCKET, libc::SO_ATTACH_FILTER | libc::SO_LOCK_FILTER | libc::SO_ATTACH_BPF | libc::SO_ATTACH_REUSEPORT_CBPF | libc::SO_ATTACH_REUSEPORT_EBPF) => {
                crate::report_strict_failure("unsupported BPF `setsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (libc::SOL_SOCKET, libc::SO_BINDTODEVICE) => {
                crate::report_strict_failure("unsupported SO_BINDTODEVICE `setsockopt` option requested");
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (libc::SOL_SOCKET, libc::SO_BROADCAST) => {
                crate::report_strict_failure("unsupported SO_BROADCAST `setsockopt` option requested");
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
            (SOL_SCTP, SCTP_SOCKOPT_BINDX_ADD | SCTP_SOCKOPT_BINDX_REM | SCTP_SOCKOPT_CONNECTX_OLD | SCTP_GET_PEER_ADDRS | SCTP_GET_LOCAL_ADDRS | SCTP_SOCKOPT_CONNECTX | SCTP_SOCKOPT_CONNECTX3 | SCTP_GET_ASSOC_STATS | SCTP_PR_SUPPORTED | libc::SCTP_I_WANT_MAPPED_V4_ADDR | libc::SCTP_FRAGMENT_INTERLEAVE) => {
                // ignore the received value
                0
            }
            (SOL_SCTP, libc::SCTP_RTOINFO | libc::SCTP_ASSOCINFO | libc::SCTP_INITMSG | libc::SCTP_NODELAY | libc::SCTP_AUTOCLOSE | libc::SCTP_DISABLE_FRAGMENTS | libc::SCTP_PEER_ADDR_PARAMS | libc::SCTP_DEFAULT_SEND_PARAM | libc::SCTP_EVENTS | libc::SCTP_MAXSEG) => {
                // Ignore received value
                0
            }
            (SOL_SCTP, libc::SCTP_SET_PEER_PRIMARY_ADDR | libc::SCTP_PRIMARY_ADDR) => {
                // Ignoring received value would cause issues
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, libc::SCTP_STATUS | libc::SCTP_GET_PEER_ADDR_INFO) => {
                // readonly option
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
            (SOL_SCTP, _) => panic!("Unrecognized socket option: SOL_SCTP, optname {}", optname),
            (libc::SOL_IPV6, libc::IPV6_V6ONLY) => {
                0 // Ignore received value
            }
            (libc::SOL_IPV6, _) => panic!("Unrecognized socket option: SOL_IPV6, optname {}", optname),
            _ => panic!("Unrecognized socket option: level {}, optname {}", level, optname),
        }
    }
}
