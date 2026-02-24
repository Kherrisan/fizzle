use std::ffi::{CStr, CString};
use std::mem::{self, MaybeUninit};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::slice;
use std::str::FromStr;

use bitflags::bitflags;
use fizzle_common::io::SockAddr;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

// TODO: upstream
const AI_IDN: libc::c_int = 0x0040;
const AI_CANONIDN: libc::c_int = 0x0080;
const AI_IDN_ALLOW_UNASSIGNED: libc::c_int = 0x0100;
const AI_IDN_USE_STD3_ASCII_RULES: libc::c_int = 0x0200;

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct GetAddrInfoFlags: libc::c_int {
        /// Socket address is intended for `bind`.
        const PASSIVE = libc::AI_PASSIVE;
        /// Request for canonical name.
        const CANON_NAME = libc::AI_CANONNAME;
        /// Don't use name resolution.
        const NUMERICHOST = libc::AI_NUMERICHOST;
        /// IPv4 mapped addresses are acceptable.
        const V4MAPPED = libc::AI_V4MAPPED;
        /// Return IPv4-mapped and IPv6 addresses.
        const ALL = libc::AI_ALL;
        /// Use configuration of this host to choose returned address type.
        const ADDRCONFIG = libc::AI_ADDRCONFIG;
        /// IDN-encode input (assuming it is encoded in the current locale's character set).
        const IDN = AI_IDN;
        /// Translate canonical name from IDN format.
        const CANON_IDN = AI_CANONIDN;
        const IDN_ALLOW_UNASSIGNED = AI_IDN_ALLOW_UNASSIGNED;
        const IDN_USE_STD3_ASCII_RULES = AI_IDN_USE_STD3_ASCII_RULES;
        /// Don't use your name resolution.
        const NUMERICSERV = libc::AI_NUMERICSERV;
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

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if self.node.is_none() && self.service.is_none() {
            return Outcome::Error((Errno::EINVAL, libc::EAI_NONAME));
        }

        let mut template: libc::addrinfo = unsafe { MaybeUninit::zeroed().assume_init() };

        if self.hint_flags.contains(GetAddrInfoFlags::CANON_NAME) {
            template.ai_canonname = match self.node {
                Some(cstr) => CString::into_raw(cstr.to_owned()),
                None => return Outcome::Error((Errno::EINVAL, libc::EAI_BADFLAGS)),
            };
        }

        template.ai_family = self.hint_family;
        template.ai_socktype = self.hint_socktype;
        template.ai_protocol = self.hint_protocol;

        if template.ai_family == 0 {
            template.ai_family = libc::AF_UNSPEC;
        }

        if template.ai_protocol == 0 {
            template.ai_protocol = match template.ai_socktype {
                0 => 0,
                libc::SOCK_STREAM => libc::IPPROTO_TCP,
                libc::SOCK_DGRAM => libc::IPPROTO_UDP,
                _ => unimplemented!(),
            };
        }

        let port = match self.service {
            None => 0,
            Some(service) => {
                let service = service.to_str().unwrap();

                match service.parse() {
                    Ok(p) => p,
                    _ => match service {
                        "ftp-data" => 20,
                        "ftp" => 21,
                        "ssh" => 22,
                        "smtp" => 25,
                        "http" => 80,
                        "pop3" => 110,
                        "snmp" => 161,
                        "ldap" => 389,
                        "https" => 443,
                        "syslog" => 514,
                        "rtsp" => 554,
                        "ftps" => 990,
                        "mysql" => 3306,
                        "daap" => 3689,
                        "postgresql" => 5432,
                        _ => panic!("unrecognized service `{:?}` in getaddrinfo()", service),
                    },
                }
            }
        };

        let mut first_record = None;

        match self.node {
            None => {
                // NOTE: code taken directly from `c-ward` crate (MIT+Apache2)

                // Decide which families to emit records for.
                let v6_v4 = [libc::AF_INET6, libc::AF_INET];
                let one_family = [template.ai_family];
                let ai_families = match template.ai_family {
                    libc::AF_UNSPEC => &v6_v4[..],
                    libc::AF_INET | libc::AF_INET6 => &one_family[..],
                    _ => {
                        return Outcome::Error((Errno::EILSEQ, libc::EAI_SERVICE));
                    }
                };

                // Decide which socket types to emit records for.
                let stream_dgram = [libc::SOCK_STREAM, libc::SOCK_DGRAM];
                let one_socktype = [template.ai_socktype];
                let ai_socktypes = match template.ai_socktype {
                    0 => &stream_dgram[..],
                    libc::SOCK_STREAM | libc::SOCK_DGRAM => &one_socktype[..],
                    _ => return Outcome::Error((Errno::EILSEQ, libc::EAI_SERVICE)),
                };

                // Emit the records.
                for ai_family in ai_families {
                    for ai_socktype in ai_socktypes {
                        let mut info = Box::new(template.clone());

                        info.ai_socktype = *ai_socktype;
                        info.ai_family = *ai_family;

                        let mut storage: Box<libc::sockaddr_storage> =
                            Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
                        let storage_ptr: *mut libc::sockaddr_storage = &mut *storage;
                        let storage_slice = unsafe {
                            slice::from_raw_parts_mut(
                                storage_ptr.cast(),
                                mem::size_of::<libc::sockaddr_storage>(),
                            )
                        };

                        let is_passive = template.ai_flags & libc::AI_PASSIVE == libc::AI_PASSIVE;
                        let len = match *ai_family {
                            libc::AF_INET => {
                                let addr = if is_passive {
                                    Ipv4Addr::UNSPECIFIED
                                } else {
                                    Ipv4Addr::LOCALHOST
                                };
                                SockAddr::Ipv4(SocketAddrV4::new(addr, port)).encode(storage_slice)
                            }
                            libc::AF_INET6 => {
                                let addr = if is_passive {
                                    Ipv6Addr::UNSPECIFIED
                                } else {
                                    Ipv6Addr::LOCALHOST
                                };
                                SockAddr::Ipv6(SocketAddrV6::new(addr, port, 0, 0))
                                    .encode(storage_slice)
                            }
                            _ => unreachable!(),
                        };
                        info.ai_addr = Box::into_raw(storage).cast();
                        info.ai_addrlen = len as u32;

                        let Some(next_record) = first_record.as_mut() else {
                            first_record = Some(info);
                            continue;
                        };

                        let mut next_ptr: *mut libc::addrinfo = &mut **next_record;

                        unsafe {
                            while !(*next_ptr).ai_next.is_null() {
                                next_ptr = (*next_ptr).ai_next;
                            }

                            (*next_ptr).ai_next = Box::into_raw(info).cast();
                        }
                    }
                }
            }
            Some(node) => {
                // Otherwise, we have a `node`; prepare to work with it.
                let host = match node.to_str() {
                    Ok(host) => host,
                    Err(_) => return Outcome::Error((Errno::EILSEQ, libc::EAI_SYSTEM)),
                };

                if let Ok(addr) = IpAddr::from_str(host) {
                    let mut info = Box::new(template.clone());

                    let sockaddr = match addr {
                        IpAddr::V4(v4_addr) => {
                            if info.ai_family == libc::AF_UNSPEC {
                                info.ai_family = libc::AF_INET;
                            }
                            if info.ai_family != libc::AF_INET {
                                return Outcome::Error((Errno::EIO, libc::EAI_NONAME));
                            }

                            SockAddr::Ipv4(SocketAddrV4::new(v4_addr, port))
                        }
                        IpAddr::V6(v6_addr) => {
                            if info.ai_family == libc::AF_UNSPEC {
                                info.ai_family = libc::AF_INET6;
                            }
                            if info.ai_family != libc::AF_INET6 {
                                return Outcome::Error((Errno::EIO, libc::EAI_NONAME));
                            }

                            SockAddr::Ipv6(SocketAddrV6::new(v6_addr, port, 0, 0))
                        }
                    };

                    let mut storage: Box<libc::sockaddr_storage> =
                        Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
                    let storage_ptr: *mut libc::sockaddr_storage = &mut *storage;
                    let storage_slice = unsafe {
                        slice::from_raw_parts_mut(
                            storage_ptr.cast(),
                            mem::size_of::<libc::sockaddr_storage>(),
                        )
                    };

                    let len = sockaddr.encode(storage_slice);

                    info.ai_addr = Box::into_raw(storage).cast();
                    info.ai_addrlen = len as u32;

                    return Outcome::Success(info);

                } else if template.ai_flags & libc::AI_NUMERICHOST == libc::AI_NUMERICHOST {
                    return Outcome::Error((Errno::EIO, libc::EAI_NONAME))
                } else {

                    // Otherwise, mock lookups for `node`

                    let Ok(node) = node.to_str() else {
                        log::warn!("non-UTF8 input `node` to getaddrinfo()");
                        return Outcome::Error((Errno::EINVAL, libc::EAI_NONAME));
                    };

                    let mut addrs = Vec::new();

                    if template.ai_family == libc::AF_UNSPEC || template.ai_family == libc::AF_INET {
                        match node {
                            "localhost" => addrs.push(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
                            _ => {
                                log::warn!("getaddrinfo() with node `{}` has no IPv4 addrs assigned--giving default 192.168.0.1", node);
                                addrs.push(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)));
                            }
                        }
                    }

                    if template.ai_family == libc::AF_UNSPEC || template.ai_family == libc::AF_INET6 {
                        match node {
                            "localhost" => {
                                addrs.push(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)))
                            }
                            _ => {
                                log::warn!("getaddrinfo() with node `{}` has no IPv6 addrs assigned--giving default [::10]", node);
                                addrs.push(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 10)));
                            }
                        }
                    }

                    for addr in addrs {
                        let mut info = Box::new(template.clone());

                        let mut storage: Box<libc::sockaddr_storage> =
                            Box::new(unsafe { MaybeUninit::zeroed().assume_init() });
                        let storage_ptr: *mut libc::sockaddr_storage = &mut *storage;
                        let storage_slice = unsafe {
                            slice::from_raw_parts_mut(
                                storage_ptr.cast(),
                                mem::size_of::<libc::sockaddr_storage>(),
                            )
                        };

                        match addr {
                            IpAddr::V4(v4) => {
                                if template.ai_family == libc::AF_UNSPEC
                                    || template.ai_family == libc::AF_INET
                                {
                                    let len = SockAddr::Ipv4(SocketAddrV4::new(v4, port))
                                        .encode(storage_slice);
                                    info.ai_addr = Box::into_raw(storage).cast();
                                    info.ai_addrlen = len as u32;
                                    info.ai_family = libc::AF_INET;
                                }
                            }
                            IpAddr::V6(v6) => {
                                if template.ai_family == libc::AF_UNSPEC
                                    || template.ai_family == libc::AF_INET6
                                {
                                    let len = SockAddr::Ipv6(SocketAddrV6::new(v6, port, 0, 0))
                                        .encode(storage_slice);
                                    info.ai_addr = Box::into_raw(storage).cast();
                                    info.ai_addrlen = len.try_into().unwrap();
                                    info.ai_family = libc::AF_INET6;
                                }
                            }
                        }

                        let Some(next_record) = first_record.as_mut() else {
                            first_record = Some(info);
                            continue;
                        };

                        let mut next_ptr: *mut libc::addrinfo = &mut **next_record;

                        unsafe {
                            while !(*next_ptr).ai_next.is_null() {
                                next_ptr = (*next_ptr).ai_next;
                            }

                            (*next_ptr).ai_next = Box::into_raw(info).cast();
                        }
                    }
                }
            }
        }

        if let Some(record) = first_record {
            Outcome::Success(record)
        } else {
            Outcome::Error((Errno::SUCCESS, libc::EAI_FAIL))
        }
    }
}

pub struct FreeAddressInfoEvent {
    ptr: *mut libc::addrinfo,
}

impl FreeAddressInfoEvent {
    pub fn new(ptr: *mut libc::addrinfo) -> Self {
        Self { ptr }
    }
}

impl Event for FreeAddressInfoEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
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

            if !ai.ai_addr.is_null() {
                let addr = unsafe { Box::from_raw(ai.ai_addr.cast::<libc::sockaddr_storage>()) };
                drop(addr);
            }
        }

        Outcome::Success(())
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct GetNameInfoFlags: libc::c_int {
        const NAMEREQD = libc::NI_NAMEREQD;
        const DGRAM = libc::NI_DGRAM;
        const NOFQDN = libc::NI_NOFQDN;
        const NUMERICHOST = libc::NI_NUMERICHOST;
        const NUMERICSERV = libc::NI_NUMERICSERV;
    }
}

pub struct GetNameInfoEvent<'a> {
    addr: SockAddr,
    host: Option<&'a mut [MaybeUninit<u8>]>,
    service: Option<&'a mut [MaybeUninit<u8>]>,
    flags: GetNameInfoFlags,
}

impl<'a> GetNameInfoEvent<'a> {
    pub fn new(
        addr: SockAddr,
        host: Option<&'a mut [MaybeUninit<u8>]>,
        service: Option<&'a mut [MaybeUninit<u8>]>,
        flags: GetNameInfoFlags,
    ) -> Self {
        Self {
            addr,
            host,
            service,
            flags,
        }
    }
}

impl Event for GetNameInfoEvent<'_> {
    type Success = ();
    type Error = libc::c_int;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if let Some(service_slice) = self.service.as_mut() {
            let port = match &self.addr {
                SockAddr::Ipv4(v4) => v4.port(),
                SockAddr::Ipv6(v6) => v6.port(),
                _ => unreachable!(),
            };

            if self.flags.contains(GetNameInfoFlags::NUMERICSERV) {
                let port_str = port.to_string();
                let port_slice = port_str.as_bytes();

                if service_slice.len() < port_slice.len() + 1 {
                    return Outcome::Error(libc::EAI_OVERFLOW);
                }

                for i in 0..port_slice.len() {
                    service_slice[i].write(port_slice[i]);
                }

                service_slice[port_slice.len()].write(b'\0');
            } else {
                let port_str = port.to_string();

                let service_str = match port {
                    20 => "ftp-data",
                    21 => "ftp",
                    22 => "ssh",
                    25 => "smtp",
                    80 => "http",
                    110 => "pop3",
                    161 => "snmp",
                    389 => "ldap",
                    443 => "https",
                    514 => "syslog",
                    554 => "rtsp",
                    990 => "ftps",
                    3306 => "mysql",
                    3689 => "daap",
                    5432 => "postgresql",
                    _ => {
                        log::warn!("getnameinfo() unknown service for port {}", port);
                        port_str.as_str()
                    }
                };

                let out_slice = service_str.as_bytes();

                if service_slice.len() < out_slice.len() + 1 {
                    return Outcome::Error(libc::EAI_OVERFLOW);
                }

                for i in 0..out_slice.len() {
                    service_slice[i].write(out_slice[i]);
                }

                service_slice[out_slice.len()].write(b'\0');
            }
        }

        if let Some(host_slice) = self.host.as_mut() {
            let ip_str = match &self.addr {
                SockAddr::Ipv4(v4) => IpAddr::V4(*v4.ip()).to_string(),
                SockAddr::Ipv6(v6) => IpAddr::V6(*v6.ip()).to_string(),
                SockAddr::Unix(un) => un.to_string(),
            };

            if self.flags.contains(GetNameInfoFlags::NUMERICHOST) {
                let ip_slice = ip_str.as_bytes();

                if host_slice.len() < ip_slice.len() + 1 {
                    return Outcome::Error(libc::EAI_OVERFLOW);
                }

                for i in 0..ip_slice.len() {
                    host_slice[i].write(ip_slice[i]);
                }
                host_slice[ip_slice.len()].write(b'\0');
            }

            let host_str = match ip_str.as_str() {
                "127.0.0.1" | "::1" => Some("localhost"),
                "192.168.0.1" | "::10" => Some("ubuntu"),
                _ => {
                    log::warn!(
                        "IP address {} had no associated hostname in getnameinfo()",
                        ip_str
                    );
                    None
                }
            };

            if let Some(host_str) = host_str {
                let s = host_str.as_bytes();

                if host_slice.len() < s.len() + 1 {
                    return Outcome::Error(libc::EAI_OVERFLOW);
                }

                for i in 0..s.len() {
                    host_slice[i].write(s[i]);
                }
                host_slice[s.len()].write(b'\0');
            } else {
                if self.flags.contains(GetNameInfoFlags::NAMEREQD) {
                    return Outcome::Error(libc::EAI_NONAME);
                }

                let ip_slice = ip_str.as_bytes();

                if host_slice.len() < ip_slice.len() + 1 {
                    return Outcome::Error(libc::EAI_OVERFLOW);
                }

                for i in 0..ip_slice.len() {
                    host_slice[i].write(ip_slice[i]);
                }
                host_slice[ip_slice.len()].write(b'\0');
            }
        }

        Outcome::Success(())
    }
}
