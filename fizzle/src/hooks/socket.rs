use std::mem::MaybeUninit;
use std::{cmp, mem, slice};

use crate::errno::Errno;
use crate::handlers::descriptor::Descriptor;
use crate::handlers::socket::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

use fizzle_common::io::{AddressFamily, SockAddr, SocketType, TransportProtocol};

hook_macros::hook! {
    unsafe fn socket(
        domain: libc::c_int,
        socktype: libc::c_int,
        protocol: libc::c_int
    ) -> libc::c_int => fizzle_socket(ctx) {

        crate::strace!("socket(domain={}, socktype={}, protocol={}) -> ...", domain, socktype, protocol);

        let nonblocking = (socktype & libc::SOCK_NONBLOCK) != 0;
        let cloexec = (socktype & libc::SOCK_CLOEXEC) != 0;
        let socktype = socktype & !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC);

        let domain = match domain {
            libc::AF_INET => AddressFamily::Ipv4,
            libc::AF_INET6 => AddressFamily::Ipv6,
            libc::AF_UNIX => AddressFamily::Unix,
            libc::AF_NETLINK => AddressFamily::Netlink,
            _ => unimplemented!("unsupported socket domain {}", domain),
        };

        let socket_type = match socktype {
            libc::SOCK_STREAM => SocketType::Stream,
            libc::SOCK_SEQPACKET => SocketType::SeqPacket,
            libc::SOCK_DGRAM => SocketType::Datagram,
            libc::SOCK_RAW => SocketType::Raw,
            _ => unimplemented!("unsupported socket type {}", socktype),
        };

        let protocol = match (domain, socket_type, protocol) {
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, SocketType::Stream, 0) => TransportProtocol::Tcp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, SocketType::Datagram, 0) => TransportProtocol::Udp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, _, libc::IPPROTO_TCP) => TransportProtocol::Tcp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, _, libc::IPPROTO_UDP) => TransportProtocol::Udp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, _, libc::IPPROTO_SCTP) => TransportProtocol::Sctp,
            (AddressFamily::Unix, _, 0) => TransportProtocol::Unix,
            _ => unimplemented!("unsupported socket domain/protocol pair {}, {}", domain, protocol),
        };

        let socket_create_event = SocketCreateEvent {
            domain,
            socket_type,
            protocol,
            nonblocking,
            cloexec
        };

        match Scheduler::handle_event(&mut ctx, socket_create_event) {
            Ok(descriptor_id) => {
                let fd = descriptor_id.as_raw_fd();
                crate::strace!("socket(domain={}, socktype={}, protocol={}) -> {}", domain, socktype, protocol, fd);
                fd
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn socketpair(
        domain: libc::c_int,
        ty: libc::c_int,
        protocol: libc::c_int,
        sv: *mut [libc::c_int; 2]
    ) -> libc::c_int => fizzle_clearerr(ctx) {

        let Some(sv) = sv.as_mut() else {
            Errno::EINVAL.set_errno();
            return -1
        };

        crate::strace!("socketpair(domain={}, ty={}, protocol={}, sv={:?}) -> ...", domain, ty, protocol, sv);

        let nonblocking = (ty & libc::SOCK_NONBLOCK) != 0;
        let cloexec = (ty & libc::SOCK_CLOEXEC) != 0;
        let socktype = ty & !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC);

        let domain = match domain {
            libc::AF_INET => AddressFamily::Ipv4,
            libc::AF_INET6 => AddressFamily::Ipv6,
            libc::AF_UNIX => AddressFamily::Unix,
            _ => unimplemented!("unsupported socket domain {}", domain),
        };

        let socket_type = match socktype {
            libc::SOCK_STREAM => SocketType::Stream,
            libc::SOCK_SEQPACKET => SocketType::SeqPacket,
            libc::SOCK_DGRAM => SocketType::Datagram,
            libc::SOCK_RAW => SocketType::Raw,
            _ => unimplemented!("unsupported socket type {}", socktype),
        };

        let protocol = match (domain, protocol) {
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, 0 | libc::IPPROTO_TCP) => TransportProtocol::Tcp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, libc::IPPROTO_UDP) => TransportProtocol::Udp,
            (AddressFamily::Ipv4 | AddressFamily::Ipv6, libc::IPPROTO_SCTP) => TransportProtocol::Sctp,
            (AddressFamily::Unix, 0) => TransportProtocol::Unix,
            _ => unimplemented!("unsupported socket domain/protocol pair {}, {}", domain, protocol),
        };

        // TODO: add support for AF_TIPC
        if domain != AddressFamily::Unix {
            Errno::EAFNOSUPPORT.set_errno();
            return -1
        }

        let socket_create_event = SocketCreatePairEvent {
            domain,
            socket_type,
            protocol,
            nonblocking,
            cloexec
        };

        match Scheduler::handle_event(&mut ctx, socket_create_event) {
            Ok((descriptor1, descriptor2)) => {
                sv[0] = descriptor1.as_raw_fd();
                sv[1] = descriptor2.as_raw_fd();

                crate::strace!("socketpair(domain={}, socktype={}, protocol={}, sv={:?}) -> {}", domain, socktype, protocol, sv, 0);
                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn bind(
        fd: libc::c_int,
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::c_int => fizzle_bind(ctx) {
        // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
        let addr_bytes = slice::from_raw_parts(addr as *const u8, addrlen as usize);

        crate::strace!("bind(fd={}, addr={:?}, addrlen={} ({:?})) -> ...", fd, addr, addrlen, addr_bytes);

        let Ok(sockaddr) = SockAddr::decode(addr_bytes) else {
            crate::strace!("bind(fd={}, addr={:?}, addrlen={} ({:?})) -> -1 (EINVAL)", fd, addr, addrlen, addr_bytes);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, SocketBindEvent::new(Descriptor::from_raw_fd(fd), sockaddr.clone())) {
            Ok(()) => {
                crate::strace!("bind(fd={}, addr={:?}, addrlen={} ({:?})) -> 0", fd, addr, addrlen, sockaddr);
                0
            },
            Err(e) => {
                crate::strace!("bind(fd={}, addr={:?}, addrlen={} ({:?})) -> 0", fd, addr, addrlen, sockaddr);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn listen(
        fd: libc::c_int,
        backlog: libc::c_int
    ) -> libc::c_int => fizzle_listen(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("listen(fd={}, backlog={}) -> ...", fd, backlog);

        match Scheduler::handle_event(&mut ctx, SocketListenEvent::new(descriptor_id, backlog)) {
            Ok(()) => {
                crate::strace!("listen(fd={}, backlog={}) -> 0", fd, backlog);
                0
            },
            Err(e) => {
                crate::strace!("listen(fd={}, backlog={}) -> -1 ({})", fd, backlog, e);
                e.set_errno();
                -1
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
        // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
        let addr_bytes = slice::from_raw_parts(addr as *const u8, addrlen as usize);
        let descriptor_id = Descriptor::from_raw_fd(fd);

        let Ok(sockaddr) = SockAddr::decode(addr_bytes) else {
            crate::strace!("connect(fd={}, addr={:?}, addrlen={} ({:?})) -> -1 (EINVAL)", fd, addr, addrlen, addr_bytes);
            Errno::EINVAL.set_errno();
            return -1
        };

        crate::strace!("connect(fd={}, addr={:?}, addrlen={} ({:?})) -> ...", fd, addr, addrlen, sockaddr);

        match Scheduler::handle_event(&mut ctx, SocketConnectEvent::new(descriptor_id, sockaddr.clone())) {
            Ok(()) => {
                crate::strace!("connect(fd={}, addr={:?}, addrlen={} ({:?})) -> 0", fd, addr, addrlen, sockaddr);
                0
            },
            Err(e) => {
                crate::strace!("connect(fd={}, addr={:?}, addrlen={} ({:?})) -> -1 ({})", fd, addr, addrlen, sockaddr, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn accept(
        fd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_accept(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("accept(fd={}, addr={:?}, addrlen={:?}) -> ...", fd, addr, addrlen);

        match Scheduler::handle_event(&mut ctx, SocketAcceptEvent::new(descriptor_id, false, false)) {
            Ok((descriptor_id, accept_addr)) => {
                let accept_fd = descriptor_id.as_raw_fd();

                if !addr.is_null() && !addrlen.is_null() {
                    // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
                    let addr_bytes = slice::from_raw_parts_mut(addr as *mut MaybeUninit<u8>, addrlen as usize);
                    *addrlen = accept_addr.encode(addr_bytes) as u32;

                    crate::strace!("accept(fd={}, addr={:?}, addrlen={:?} ({})) -> {}", fd, addr, addrlen, accept_addr, accept_fd);

                } else {
                    crate::strace!("accept(fd={}, addr={:?}, addrlen={:?}) -> {}", fd, addr, addrlen, accept_fd);
                }

                accept_fd
            },
            Err(e) => {
                crate::strace!("accept(fd={}, addr={:?}, addrlen={:?}) -> -1 ({})", fd, addr, addrlen, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn accept4(
        fd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_accept4(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);
        let nonblocking = flags & libc::SOCK_NONBLOCK > 0;
        let cloexec = flags & libc::SOCK_CLOEXEC > 0;

        crate::strace!("accept4(fd={}, addr={:?}, addrlen={:?}, flags={}) -> ...", fd, addr, addrlen, flags);

        if flags & !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC) > 0 {
            crate::strace!("accept4(fd={}, addr={:?}, addrlen={:?}, flags={}) -> -1 (EINVAL)", fd, addr, addrlen, flags);
            Errno::EINVAL.set_errno();
            return -1
        }

        let flags_fmt = match (nonblocking, cloexec) {
            (false, false) => "0",
            (false, true) => "SOCK_CLOEXEC",
            (true, false) => "SOCK_NONBLOCK",
            (true, true) => "SOCK_NONBLOCK|SOCK_CLOEXEC",
        };

        match Scheduler::handle_event(&mut ctx, SocketAcceptEvent::new(descriptor_id, false, false)) {
            Ok((descriptor_id, accept_addr)) => {
                let accept_fd = descriptor_id.as_raw_fd();

                if !addr.is_null() && !addrlen.is_null() {
                    // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
                    let addr_bytes = slice::from_raw_parts_mut(addr as *mut MaybeUninit<u8>, addrlen as usize);
                    *addrlen = accept_addr.encode(addr_bytes) as u32;

                    crate::strace!("accept4(fd={}, addr={:?}, addrlen={:?} ({}), flags={}) -> {}", fd, addr, addrlen, accept_addr, flags_fmt, accept_fd);

                } else {
                    crate::strace!("accept4(fd={}, addr={:?}, addrlen={:?}, flags={}) -> {}", fd, addr, addrlen, flags_fmt, accept_fd);
                }

                accept_fd
            },
            Err(e) => {
                crate::strace!("connect4(fd={}, addr={:?}, addrlen={:?}, flags={}) -> -1 ({})", fd, addr, addrlen, flags_fmt, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn getsockname(
        sockfd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_getsockname(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(sockfd);

        crate::strace!("getsockname(sockfd={}, addr={:?}, addrlen={:?}) -> ...", sockfd, addr, addrlen);

        match Scheduler::handle_event(&mut ctx, SocketGetNameEvent::new(descriptor_id)) {
            Ok(socket_addr) => {

                if !addr.is_null() && !addrlen.is_null() {
                    // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
                    let addr_bytes = slice::from_raw_parts_mut(addr as *mut MaybeUninit<u8>, addrlen as usize);

                    match socket_addr {
                        Ok(sockaddr) => {
                            crate::strace!("getsockname(sockfd={}, addr={:?}, addrlen={:?} ({:?})) -> 0", sockfd, addr, addrlen, sockaddr);
                            *addrlen = sockaddr.encode(addr_bytes) as u32;
                        }
                        Err(family) => {
                            crate::strace!("getsockname(sockfd={}, addr={:?}, addrlen={:?} (<unbound>)) -> 0", sockfd, addr, addrlen);
                            addr_bytes.fill(MaybeUninit::new(0));

                            let family_bytes = family.raw().to_be_bytes().map(|i| MaybeUninit::new(i));

                            let family_bytelen = cmp::min(family_bytes.len(), addr_bytes.len());
                            addr_bytes[..family_bytelen].copy_from_slice(&family_bytes);
                        }
                    }
                } else {
                    crate::strace!("getsockname(sockfd={}, addr={:?}, addrlen={:?}) -> 0", sockfd, addr, addrlen);
                }

                0
            },
            Err(e) => {
                crate::strace!("getsockname(sockfd={}, addr={:?}, addrlen={:?}) -> 0", sockfd, addr, addrlen);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn getpeername(
        sockfd: libc::c_int,
        addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_getsockname(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(sockfd);

        crate::strace!("getpeername(sockfd={}, addr={:?}, addrlen={:?}) -> ...", sockfd, addr, addrlen);

        match Scheduler::handle_event(&mut ctx, SocketGetPeerNameEvent::new(descriptor_id)) {
            Ok(sockaddr) => {

                if !addr.is_null() && !addrlen.is_null() {
                    // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
                    let addr_bytes = slice::from_raw_parts_mut(addr as *mut MaybeUninit<u8>, addrlen as usize);

                    crate::strace!("getpeername(sockfd={}, addr={:?}, addrlen={:?} ({:?})) -> 0", sockfd, addr, addrlen, sockaddr);
                    *addrlen = sockaddr.encode(addr_bytes) as u32;
                } else {
                    crate::strace!("getpeername(sockfd={}, addr={:?}, addrlen={:?}) -> 0", sockfd, addr, addrlen);
                }

                0
            },
            Err(e) => {
                crate::strace!("getpeername(sockfd={}, addr={:?}, addrlen={:?}) -> 0", sockfd, addr, addrlen);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn getsockopt(
        sockfd: libc::c_int,
        level: libc::c_int,
        optname: libc::c_int,
        optval: *mut libc::c_void,
        optlen: *mut libc::socklen_t
    ) -> libc::c_int => fizzle_getsockopt(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(sockfd);

        let opt_level = match level {
            libc::SOL_SOCKET => OptLevel::Socket,
            libc::SOL_IP => OptLevel::Ip,
            libc::SOL_IPV6 => OptLevel::Ipv6,
            SOL_SCTP => OptLevel::Sctp,
            libc::SOL_TCP => OptLevel::Tcp,
            _ => {
                unimplemented!("`getsockopt()` optlevel {}", level)
            }
        };

        crate::strace!("getsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> ...", sockfd, opt_level, optname, optval, optlen);

        let input = match (opt_level, optname) {
            (OptLevel::Sctp, libc::SCTP_ASSOCINFO) => {
                if optval.is_null() || optlen.is_null() || (*optlen as usize) < mem::size_of::<SctpAssocParams>() {
                    Errno::EINVAL.set_errno();
                    return -1
                }

                let assoc_id = (*optval.cast::<SctpAssocParams>()).sasoc_assoc_id;

                OptInput::SctpAssocId(assoc_id)
            }
            (OptLevel::Sctp, libc::SCTP_RTOINFO) => {
                if optval.is_null() || optlen.is_null() || (*optlen as usize) < mem::size_of::<SctpRtoInfo>() {
                    Errno::EINVAL.set_errno();
                    return -1
                }

                let assoc_id = (*optval.cast::<SctpRtoInfo>()).srto_assoc_id;
                OptInput::SctpAssocId(assoc_id)
            }
            (OptLevel::Sctp, libc::SCTP_PEER_ADDR_PARAMS) => {
                if optval.is_null() || optlen.is_null() || *optlen < 4 + mem::size_of::<libc::sockaddr_storage>() as u32 {
                    Errno::EINVAL.set_errno();
                    return -1
                }
                let params = &*optval.cast::<SctpPeerAddrParams>();
                let assoc_id = params.spp_assoc_id;
                let addr = params.spp_address;
                OptInput::SctpPeerAddrParams(assoc_id, addr)
            }
            _ => OptInput::None,
        };

        match Scheduler::handle_event(&mut ctx, SocketGetOptionEvent::new(descriptor_id, opt_level, optname, input)) {
            Ok(ret) => {
                crate::strace!("getsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);

                if !optval.is_null() && !optlen.is_null() {
                    // SAFETY: caller ensures addr points to a valid buffer of `adderlen` bytes.
                    let opt_bytes = slice::from_raw_parts_mut(optval as *mut MaybeUninit<u8>, optlen as usize);


                    *optlen = ret.encode(opt_bytes) as u32;
                }

                0
            },
            Err(e) => {
                crate::strace!("getsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 ({})", sockfd, opt_level, optname, optval, optlen, e);
                e.set_errno();
                -1
            },
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
        let descriptor_id = Descriptor::from_raw_fd(sockfd);

        crate::strace!("setsockopt(sockfd={}, level={}, optname={}, optval={:?}, optlen={:?}) -> ...", sockfd, level, optname, optval, optlen);

        let opt_level = match level {
            libc::SOL_SOCKET => OptLevel::Socket,
            libc::SOL_IP => OptLevel::Ip,
            libc::SOL_IPV6 => OptLevel::Ipv6,
            SOL_SCTP => OptLevel::Sctp,
            libc::SOL_TCP => OptLevel::Tcp,
            _ => {
                unimplemented!("`setsockopt()` optlevel {}", level)
            }
        };

        let input = match (opt_level, optname) {
            (OptLevel::Ip, libc::IP_TOS | libc::IP_MTU_DISCOVER | libc::IP_DROP_MEMBERSHIP | libc::IP_OPTIONS | libc::IP_MULTICAST_LOOP | libc::IP_ADD_MEMBERSHIP | libc::IP_MULTICAST_ALL | libc::IP_MULTICAST_TTL | libc::IP_FREEBIND | libc::IP_RECVERR) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                return 0
            }
            (OptLevel::Ip, _) => {
                log::error!("Unrecognized socket option: SOL_IP, optname {}", optname);
                panic!("Unrecognized socket option: SOL_IP, optname {}", optname);
            }
            // Pretend to support (but don't)
            (OptLevel::Tcp, libc::TCP_NODELAY | libc::TCP_MAXSEG | libc::TCP_USER_TIMEOUT | libc::TCP_FASTOPEN) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                return 0
            }
            (OptLevel::Tcp, libc::TCP_KEEPIDLE | libc::TCP_KEEPCNT | libc::TCP_KEEPINTVL | libc::TCP_SYNCNT) => return 0,
            (OptLevel::Tcp, _) => {
                panic!("Unrecognized socket option: SOL_TCP, optname {}", optname);
            }
            // Socket options that are readonly
            (OptLevel::Socket, libc::SO_ACCEPTCONN | libc::SO_DOMAIN | libc::SO_ERROR | libc::SO_PROTOCOL) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                Errno::EINVAL.set_errno();
                return -1
            }
            // Socket options that we pretend to support (but don't)
            (OptLevel::Socket, libc::SO_KEEPALIVE | libc::SO_OOBINLINE | libc::SO_PRIORITY | libc::SO_RCVBUF | libc::SO_SNDLOWAT | libc::SO_RCVLOWAT | libc::SO_RCVTIMEO | libc::SO_SNDTIMEO | libc::SO_REUSEADDR | libc::SO_ZEROCOPY | libc::SO_SNDBUF) => { // TODO: configure SO_SNDBUF
                // Ignore received value
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                return 0
            }
            (OptLevel::Socket, libc::SO_REUSEPORT) => {
                if (optlen as usize) < mem::size_of::<libc::c_int>() {
                    crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                    Errno::EINVAL.set_errno();
                    return -1
                }

                let reuse = match *(optval.cast::<libc::c_int>()) {
                    0 => false,
                    1 => true,
                    _ => {
                        crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                        Errno::EINVAL.set_errno();
                        return -1
                    }
                };

                SocketOption::SocketReusePort(reuse)
            }
            (OptLevel::Socket, libc::SO_ATTACH_FILTER | libc::SO_LOCK_FILTER | libc::SO_ATTACH_BPF | libc::SO_ATTACH_REUSEPORT_CBPF | libc::SO_ATTACH_REUSEPORT_EBPF) => {
                log::error!("unsupported BPF `setsockopt` option requested");
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                Errno::EINVAL.set_errno();
                return -1
            }
            (OptLevel::Socket, libc::SO_BINDTODEVICE) => {
                log::error!("unsupported SO_BINDTODEVICE `setsockopt` option requested");
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                Errno::EINVAL.set_errno();
                return -1
            }
            (OptLevel::Socket, libc::SO_BROADCAST) => {
                log::error!("unsupported SO_BROADCAST `setsockopt` option requested");
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                Errno::EINVAL.set_errno();
                return -1
            }
            (OptLevel::Socket, libc::SO_LINGER) => {
                // Ignore received value
                return 0
            }
            // TODO: implement SO_RXQ_OVFL, SO_TIMESTAMP, when implementing `cmsg`s
            (OptLevel::Socket, _) => {
                log::error!("unsupported SOL_SOCKET option {}", optname);
                panic!("Unrecognized socket option: SOL_SOCKET, optname {}", optname);
            }
            (OptLevel::Sctp, SCTP_SOCKOPT_BINDX_ADD) => {
                log::info!("Binding SCTP socket with SCTP_SOCKOPT_BINDX_ADD");
                let addr_bytes = slice::from_raw_parts(optval.cast::<u8>(), optlen as usize);
                let Ok(addr) = SockAddr::decode(addr_bytes) else {
                    log::error!("invalid sockaddr for SCTP_SOCKOPT_BINDX_ADD");
                    crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                    Errno::EINVAL.set_errno();
                    return -1
                };

                return match Scheduler::handle_event(&mut ctx, SocketBindEvent::new(Descriptor::from_raw_fd(sockfd), addr.clone())) {
                    Ok(()) => {
                        crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}) -> 0", sockfd, opt_level, optname, addr);
                        0
                    },
                    Err(e) => {
                        crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}) -> -1 ({})", sockfd, opt_level, optname, addr, e);
                        e.set_errno();
                        -1
                    },
                }
            }
            (OptLevel::Sctp, SCTP_SOCKOPT_BINDX_REM | SCTP_SOCKOPT_CONNECTX_OLD | SCTP_GET_PEER_ADDRS | SCTP_GET_LOCAL_ADDRS | SCTP_SOCKOPT_CONNECTX | SCTP_SOCKOPT_CONNECTX3 | SCTP_GET_ASSOC_STATS | SCTP_PR_SUPPORTED | libc::SCTP_I_WANT_MAPPED_V4_ADDR | libc::SCTP_FRAGMENT_INTERLEAVE) => {
                // ignore the received value
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                return 0
            }
            (OptLevel::Sctp, libc::SCTP_RTOINFO | libc::SCTP_ASSOCINFO | libc::SCTP_INITMSG | libc::SCTP_NODELAY | libc::SCTP_AUTOCLOSE | libc::SCTP_DISABLE_FRAGMENTS | libc::SCTP_PEER_ADDR_PARAMS | libc::SCTP_DEFAULT_SEND_PARAM | libc::SCTP_EVENTS | libc::SCTP_MAXSEG) => {
                // Ignore received value
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                return 0
            }
            (OptLevel::Sctp, libc::SCTP_SET_PEER_PRIMARY_ADDR | libc::SCTP_PRIMARY_ADDR) => {
                // Ignoring received value would cause issues
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}", sockfd, opt_level, optname, optval, optlen);
                Errno::EINVAL.set_errno();
                return -1
            }
            (OptLevel::Sctp, libc::SCTP_STATUS | libc::SCTP_GET_PEER_ADDR_INFO) => {
                // readonly option
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 (EINVAL)", sockfd, opt_level, optname, optval, optlen);
                Errno::EINVAL.set_errno();
                return -1
            }
            (OptLevel::Sctp, _) => {
                log::error!("unrecognized SOL_SCTP option {}", optname);
                panic!("Unrecognized socket option: SOL_SCTP, optname {}", optname)
            }
            (OptLevel::Ipv6, libc::IPV6_V6ONLY) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                return 0 // Ignore received value
            }
            (OptLevel::Ipv6, libc::IPV6_RECVPKTINFO) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                return 0 // ignore received value TODO: implement for recvmsg()
            }
            (OptLevel::Ipv6, _) => {
                log::error!("unrecognized SOL_IP6 option {}", optname);
                panic!("Unrecognized socket option: SOL_IPV6, optname {}", optname)
            }
            _ => {
                log::error!("unrecognized option level {}, optname {}", level, optname);
                panic!("Unrecognized socket option: level {}, optname {}", level, optname)
            }
        };

        match Scheduler::handle_event(&mut ctx, SocketSetOptionEvent::new(descriptor_id, input)) {
            Ok(()) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> 0", sockfd, opt_level, optname, optval, optlen);
                0
            },
            Err(e) => {
                crate::strace!("setsockopt(sockfd={}, level={:?}, optname={}, optval={:?}, optlen={:?}) -> -1 ({})", sockfd, opt_level, optname, optval, optlen, e);
                e.set_errno();
                -1
            },
        }
    }
}
