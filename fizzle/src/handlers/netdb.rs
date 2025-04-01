use std::ffi::{CStr, CString};

use bitflags::bitflags;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct GetAddrInfoFlags: libc::c_int {
        const V4MAPPED = libc::AI_V4MAPPED;
        const ADDRCONFIG = libc::AI_ADDRCONFIG;
        const NUMERICHOST = libc::AI_NUMERICHOST;
        const PASSIVE = libc::AI_PASSIVE;
        const CANONNAME = libc::AI_CANONNAME;
        /*
        const IDN = libc::AI_IDN;
        const CANON_IDN = libc::AI_CANONIDN;
        const IDN_ALLOW_UNASSIGNED = libc::AI_IDN_ALLOW_UNASSIGNED;
        */
    }
}

pub struct GetAddressInfoEvent<'a> {
    node: Option<&'a CStr>,
    service: Option<&'a CStr>,
    hint_family: libc::c_int,
    hint_socktype: libc::c_int,
    hint_protocol: libc::c_int,
    hint_flags: GetAddrInfoFlags,

}

impl<'a> GetAddressInfoEvent<'a> {
    pub fn new(
        node: Option<&'a CStr>,
        service: Option<&'a CStr>,
        hint_family: libc::c_int,
        hint_socktype: libc::c_int,
        hint_protocol: libc::c_int,
        hint_flags: GetAddrInfoFlags,
    ) -> Self {
        Self {
            node,
            service,
            hint_family,
            hint_socktype,
            hint_protocol,
            hint_flags,
        }
    }
}

impl Event for GetAddressInfoEvent<'_> {
    type Success = Box<libc::addrinfo>;
    type Error = (Errno, libc::c_int);

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        log::error!("getaddrinfo() unimplemented");
        Outcome::Error((Errno::EINVAL, libc::EAI_FAIL))
    }
}

/*
fn help(){
    {
        // At least one of `node` or `service` must be non-null.
        if node.is_null() && service.is_null() {
            set_errno(Errno(libc::EINVAL));
            return libc::EAI_NONAME;
        }

        // If we have hints, check them for unsupported features, and then copy
        // then into the prototype `addrinfo`.
        let mut prototype: libc::addrinfo = zeroed();
        if !hints.is_null() {
            prototype = *hints;

            if prototype.ai_flags & libc::AI_CANONNAME == libc::AI_CANONNAME {
                if node.is_null() {
                    return libc::EAI_BADFLAGS;
                }
                prototype.ai_canonname = node.cast_mut();
            }

            assert_eq!(
                prototype.ai_flags & !(libc::AI_NUMERICHOST | libc::AI_NUMERICSERV | libc::AI_CANONNAME | libc::AI_PASSIVE),
                0,
                "GAI flags hint other than AI_NUMERICHOST|AI_NUMERICSERV|AI_CANONNAME|AI_PASSIVE not supported yet"
            );

            assert_eq!(
                prototype.ai_addrlen, 0,
                "GAI addrlen hint not supported yet"
            );
            assert!(
                prototype.ai_addr.is_null(),
                "GAI addr hint not supported yet"
            );
            assert!(
                prototype.ai_next.is_null(),
                "GAI next hint not supported yet"
            );
        }

        // Set a few additional fields of the prototype `addrinfo`.
        if prototype.ai_family == 0 {
            prototype.ai_family = libc::AF_UNSPEC;
        }
        if prototype.ai_protocol == 0 {
            prototype.ai_protocol = match prototype.ai_socktype {
                0 => 0,
                libc::SOCK_STREAM => libc::IPPROTO_TCP,
                libc::SOCK_DGRAM => libc::IPPROTO_UDP,
                _ => todo!("unimplemented GAI protocol {}", prototype.ai_protocol),
            };
        }

        // If we have a `service`, resolve it to a port number, otherwise use 0.
        let port = if service.is_null() {
            0
        } else {
            match resolve_service(service, &mut prototype) {
                Ok(port) => port,
                Err(err) => return err,
            }
        };

        // Prepare for `addrinfo` and `SocketAddrStorage` allocations.
        let layout = alloc::alloc::Layout::new::<libc::addrinfo>();
        let addr_layout = alloc::alloc::Layout::new::<SocketAddrStorage>();
        let mut first: *mut libc::addrinfo = null_mut();
        let mut prev: *mut libc::addrinfo = null_mut();

        // If we don't have a `node`, return `addrinfo` records of either localhost
        // or wildcard ("unspecified") addresses, following the `AI_PASSIVE` flag.
        if node.is_null() {
            // Decide which families to emit records for.
            let v6_v4 = [libc::AF_INET6, libc::AF_INET];
            let one_family = [prototype.ai_family];
            let ai_families = match prototype.ai_family {
                libc::AF_UNSPEC => &v6_v4[..],
                libc::AF_INET | libc::AF_INET6 => &one_family[..],
                _ => {
                    set_errno(Errno(libc::EILSEQ));
                    return libc::EAI_SERVICE;
                }
            };

            // Decide which socket types to emit records for.
            let stream_dgram = [libc::SOCK_STREAM, libc::SOCK_DGRAM];
            let one_socktype = [prototype.ai_socktype];
            let ai_socktypes = match prototype.ai_socktype {
                0 => &stream_dgram[..],
                libc::SOCK_STREAM | libc::SOCK_DGRAM => &one_socktype[..],
                _ => {
                    set_errno(Errno(libc::EILSEQ));
                    return libc::EAI_SERVICE;
                }
            };

            // Emit the records.
            for ai_family in ai_families {
                for ai_socktype in ai_socktypes {
                    let ptr = alloc::alloc::alloc(layout).cast::<libc::addrinfo>();
                    ptr.write(prototype);
                    let info = &mut *ptr;

                    info.ai_socktype = *ai_socktype;
                    info.ai_family = *ai_family;

                    let storage = alloc::alloc::alloc(addr_layout).cast::<SocketAddrStorage>();
                    let is_passive = prototype.ai_flags & libc::AI_PASSIVE == libc::AI_PASSIVE;
                    let len = match *ai_family {
                        libc::AF_INET => {
                            let addr = if is_passive {
                                Ipv4Addr::UNSPECIFIED
                            } else {
                                Ipv4Addr::LOCALHOST
                            };
                            SocketAddrV4::new(addr, port).write_sockaddr(storage)
                        }
                        libc::AF_INET6 => {
                            let addr = if is_passive {
                                Ipv6Addr::UNSPECIFIED
                            } else {
                                Ipv6Addr::LOCALHOST
                            };
                            SocketAddrV6::new(addr, port, 0, 0).write_sockaddr(storage)
                        }
                        _ => unreachable!(),
                    };
                    info.ai_addr = storage.cast();
                    info.ai_addrlen = len;

                    if !prev.is_null() {
                        (*prev).ai_next = ptr;
                    }
                    prev = ptr;
                    if first.is_null() {
                        first = ptr;
                    }
                }
            }
            *res = first;
            return 0;
        }

        // Otherwise, we have a `node`; prepare to work with it.
        let host = match CStr::from_ptr(node.cast()).to_str() {
            Ok(host) => host,
            Err(_) => {
                set_errno(Errno(libc::EILSEQ));
                return libc::EAI_SYSTEM;
            }
        };

        // With `AI_NUMERICHOST`, don't do any actual lookups. Just try to parse
        // `node` as an address.
        if prototype.ai_flags & libc::AI_NUMERICHOST == libc::AI_NUMERICHOST {
            let addr = match IpAddr::from_str(host) {
                Ok(addr) => addr,
                Err(_err) => {
                    set_errno(Errno(libc::EIO));
                    return libc::EAI_NONAME;
                }
            };
            match addr {
                IpAddr::V4(_) => {
                    if prototype.ai_family == libc::AF_UNSPEC {
                        prototype.ai_family = libc::AF_INET;
                    }
                    if prototype.ai_family != libc::AF_INET {
                        set_errno(Errno(libc::EIO));
                        return libc::EAI_NONAME;
                    }
                }
                IpAddr::V6(_) => {
                    if prototype.ai_family == libc::AF_UNSPEC {
                        prototype.ai_family = libc::AF_INET6;
                    }
                    if prototype.ai_family != libc::AF_INET6 {
                        set_errno(Errno(libc::EIO));
                        return libc::EAI_NONAME;
                    }
                }
            }

            let ptr = alloc::alloc::alloc(layout).cast::<libc::addrinfo>();
            ptr.write(prototype);
            let info = &mut *ptr;

            let storage = alloc::alloc::alloc(addr_layout).cast::<SocketAddrStorage>();
            let len = SocketAddr::new(addr, port).write_sockaddr(storage);
            info.ai_addr = storage.cast();
            info.ai_addrlen = len;
            *res = ptr;
            return 0;
        }

        // Otherwise, do lookups for `node`.
        match resolve_host(host, &prototype) {
            Ok(addrs) => {
                for addr in addrs {
                    let ptr = alloc::alloc::alloc(layout).cast::<libc::addrinfo>();
                    ptr.write(prototype);
                    let info = &mut *ptr;

                    match addr {
                        IpAddr::V4(v4) => {
                            if prototype.ai_family == libc::AF_UNSPEC
                                || prototype.ai_family == libc::AF_INET
                            {
                                let storage =
                                    alloc::alloc::alloc(addr_layout).cast::<SocketAddrStorage>();
                                let len = SocketAddrV4::new(v4, port).write_sockaddr(storage);
                                info.ai_addr = storage.cast();
                                info.ai_addrlen = len;
                                info.ai_family = libc::AF_INET;
                            }
                        }
                        IpAddr::V6(v6) => {
                            if prototype.ai_family == libc::AF_UNSPEC
                                || prototype.ai_family == libc::AF_INET6
                            {
                                let storage =
                                    alloc::alloc::alloc(addr_layout).cast::<SocketAddrStorage>();
                                let len = SocketAddrV6::new(v6, port, 0, 0).write_sockaddr(storage);
                                info.ai_addr = storage.cast();
                                info.ai_addrlen = len.try_into().unwrap();
                                info.ai_family = libc::AF_INET6;
                            }
                        }
                    }
                    if !prev.is_null() {
                        (*prev).ai_next = ptr;
                    }
                    prev = ptr;
                    if first.is_null() {
                        first = ptr;
                    }
                }
                *res = first;
                0
            }
            Err(err) => err,
        }
    }
}

fn resolve_host(host: &str, prototype: &libc::addrinfo) -> Result<IntoIter<IpAddr>, c_int> {
    let mut command = Command::new("getent");
    match prototype.ai_family {
        libc::AF_UNSPEC => command.arg("ahosts"),
        libc::AF_INET => command.arg("ahostsv4"),
        libc::AF_INET6 => command.arg("ahostsv6"),
        _ => {
            set_errno(Errno(libc::EIO));
            return Err(libc::EAI_SERVICE);
        }
    };
    command.arg(host);

    let output = match command.output() {
        Ok(output) => output,
        Err(_err) => {
            set_errno(Errno(libc::EIO));
            return Err(libc::EAI_SYSTEM);
        }
    };

    match output.status.code() {
        Some(0) => {}
        Some(2) => {
            // The hostname was not found. If we used `ahostsv4` or `ahostsv6`
            // then check with `ahosts`; if that succeeds, fail with
            // `EAI_ADDRFAMILY`.
            if matches!(prototype.ai_family, libc::AF_INET | libc::AF_INET6) {
                let mut command = Command::new("getent");
                command.arg("ahosts");
                command.arg(host);
                if let Ok(output) = command.output() {
                    if output.status.code() == Some(0) {
                        return Err(EAI_ADDRFAMILY);
                    }
                }
            }

            return Err(libc::EAI_NONAME);
        }
        Some(r) => panic!("unexpected exit status from `getent ahosts`: {}", r),
        None => {
            set_errno(Errno(libc::EIO));
            return Err(libc::EAI_SYSTEM);
        }
    }

    let stdout = match str::from_utf8(&output.stdout) {
        Ok(stdout) => stdout,
        Err(_err) => {
            set_errno(Errno(libc::EIO));
            return Err(libc::EAI_SYSTEM);
        }
    };

    // Iterate over the lines printed by `getent`.
    let mut hosts = Vec::new();
    for line in stdout.lines() {
        // Parse the line.
        let mut parts = line.split_ascii_whitespace();
        let addr = match parts.next() {
            Some(addr) => addr,
            None => {
                set_errno(Errno(libc::EIO));
                return Err(libc::EAI_SYSTEM);
            }
        };
        let type_ = match parts.next() {
            Some(type_) => type_,
            None => {
                set_errno(Errno(libc::EIO));
                return Err(libc::EAI_SYSTEM);
            }
        };
        // Ignore any futher parts, which would contain aliases for `host`
        // that we're uninterested in here.

        // Filter out results that don't match what's requested.
        if prototype.ai_socktype != 0 {
            let socktype_name = match prototype.ai_socktype {
                libc::SOCK_STREAM => "STREAM",
                libc::SOCK_DGRAM => "DGRAM",
                libc::SOCK_RAW => "RAW",
                _ => panic!("unsupported ai_socktype {:?}", prototype.ai_socktype),
            };
            if type_ != socktype_name {
                continue;
            }
        }

        // Parse the address.
        let addr = match IpAddr::from_str(addr) {
            Ok(addr) => addr,
            Err(_err) => {
                set_errno(Errno(libc::EIO));
                return Err(libc::EAI_SYSTEM);
            }
        };

        hosts.push(addr);
    }

    Ok(hosts.into_iter())
}

unsafe fn resolve_service(
    service: *const c_char,
    prototype: &mut libc::addrinfo,
) -> Result<u16, c_int> {
    extern "C" {
        fn getservbyname_r(
            name: *const c_char,
            proto: *const c_char,
            result_buf: *mut libc::servent,
            buf: *mut c_char,
            buflen: size_t,
            result: *mut *mut libc::servent,
        ) -> c_int;
    }

    // With `AI_NUMERICSERV`, don't do any actual lookups. Just try to parse
    // `service` as a number.
    if prototype.ai_flags & libc::AI_NUMERICSERV == libc::AI_NUMERICSERV {
        set_errno(Errno(0));
        let mut endptr: *mut c_char = null_mut();
        let number = libc::strtol(service, &mut endptr, 10);
        if endptr != service.cast_mut() && errno().0 != 0 {
            if let Ok(number) = u16::try_from(number) {
                return Ok(number);
            }
        }

        return Err(libc::EAI_NONAME);
    }

    // Do a NSS lookup for `service`.
    let proto = match prototype.ai_protocol {
        libc::IPPROTO_TCP => c"tcp".as_ptr(),
        libc::IPPROTO_UDP => c"udp".as_ptr(),
        _ => null(),
    };
    let mut servent: libc::servent = zeroed();
    let mut result = null_mut();
    match getservbyname_r(service, proto, &mut servent, null_mut(), 0, &mut result) {
        0 => {}
        libc::ENOENT => {
            let service_str = match CStr::from_ptr(service).to_str() {
                Ok(service_str) => service_str,
                Err(_) => return Err(libc::EAI_SERVICE),
            };
            let port = match u16::from_str(service_str) {
                Ok(port) => port,
                Err(_) => return Err(libc::EAI_SERVICE),
            };
            servent.s_proto = proto.cast_mut();
            servent.s_port = port.to_be().into();
        }
        _ => return Err(libc::EAI_SERVICE),
    };

    // If we don't yet know the protocol, and the query returned a protocol,
    // use that. And if we don't yet know the socktype, infer that from the
    // protocol.
    match prototype.ai_protocol {
        // Assert that the `getent` command did what we asked.
        libc::IPPROTO_TCP => assert_eq!(libc::strcmp(servent.s_proto, c"tcp".as_ptr()), 0),
        libc::IPPROTO_UDP => assert_eq!(libc::strcmp(servent.s_proto, c"udp".as_ptr()), 0),
        // Infer what we can.
        _ => {
            if !servent.s_proto.is_null() {
                if libc::strcmp(servent.s_proto, c"tcp".as_ptr()) == 0 {
                    prototype.ai_protocol = libc::IPPROTO_TCP;
                    if prototype.ai_socktype == 0 {
                        prototype.ai_socktype = libc::SOCK_STREAM;
                    }
                } else if libc::strcmp(servent.s_proto, c"udp".as_ptr()) == 0 {
                    prototype.ai_protocol = libc::IPPROTO_UDP;
                    if prototype.ai_socktype == 0 {
                        prototype.ai_socktype = libc::SOCK_DGRAM;
                    }
                } else {
                    unreachable!();
                }
            }
        }
    }

    Ok(u16::from_be(servent.s_port as u16))
}
*/






pub struct FreeAddressInfoEvent {
    ptr: *mut libc::addrinfo,
}

impl FreeAddressInfoEvent {
    pub fn new(ptr: *mut libc::addrinfo) -> Self {
        Self {
            ptr,
        }
    }
}

impl Event for FreeAddressInfoEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // Once dropped, this will free the underlying allocated addrinfo struct.
        let mut ai = unsafe { Box::from_raw(self.ptr) };

        // Once dropped, this will free the underlying allocated string.
        if !ai.ai_canonname.is_null() {
            let canonname = unsafe { CString::from_raw(ai.ai_canonname) };
            drop(canonname);
        }

        while !ai.ai_next.is_null() {
            // Implicitly drops the last `ai`
            ai = unsafe { Box::from_raw(ai.ai_next) };

            if !ai.ai_canonname.is_null() {
                let canonname = unsafe { CString::from_raw(ai.ai_canonname) };
                drop(canonname);
            }
        }

        Outcome::Success(())
    }
}


