use core::slice;
use std::ffi::CStr;
// `SocketAddr` does not use heap allocations, so it's safe for this type.
use std::fmt::Display;
use std::mem;
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::{path::FilePath, storage::Buffer};

pub const MAX_PATH_LEN: usize = 256;
pub const MAX_UNIX_ABSTRACT_LEN: usize = 107;
pub const MAX_UNIX_PATH_LEN: usize = 108;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SockAddr {
    /// An IPv4 socket address
    Ipv4(SocketAddrV4),
    /// An IPv6 socket address
    Ipv6(SocketAddrV6),
    /// A Unix socket address
    Unix(SocketAddrUnix),
    /*
    /// An AF_UNSPEC address
    Unspec,
    */
}

impl Display for SockAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ipv4(v4_addr) => v4_addr.fmt(f),
            Self::Ipv6(v6_addr) => v6_addr.fmt(f),
            Self::Unix(un_addr) => un_addr.fmt(f),
            // Self::Unspec => "<AF_UNSPEC>".fmt(f),
        }
    }
}

impl SockAddr {
    pub fn family(&self) -> AddressFamily {
        match self {
            SockAddr::Ipv4(_) => AddressFamily::Ipv4,
            SockAddr::Ipv6(_) => AddressFamily::Ipv6,
            SockAddr::Unix(_) => AddressFamily::Unix,
        }
    }

    pub fn decode(addr_bytes: &[u8]) -> Result<Self, SockAddrError> {
        let sa_family = u16::from_le_bytes(
            addr_bytes
                .get(..2)
                .ok_or(SockAddrError::InsufficientBytes)?
                .try_into()
                .unwrap(),
        );

        match sa_family as i32 {
            libc::AF_INET => {
                let sockaddr_in_bytes = addr_bytes
                    .get(..mem::size_of::<libc::sockaddr_in>())
                    .ok_or(SockAddrError::InsufficientBytes)?;
                // SAFETY: sockaddr_in can be cast from bytes as it is repr(C)
                let sockaddr_in: &libc::sockaddr_in = unsafe {
                    sockaddr_in_bytes
                        .align_to()
                        .1
                        .first()
                        .ok_or(SockAddrError::BadAlignment)?
                };

                let addr_bits = u32::from_be(sockaddr_in.sin_addr.s_addr);
                let port = u16::from_be(sockaddr_in.sin_port);

                Ok(SockAddr::Ipv4(SocketAddrV4::new(
                    Ipv4Addr::from_bits(addr_bits),
                    port,
                )))
            }
            libc::AF_INET6 => {
                let sockaddr_in6_bytes = addr_bytes
                    .get(..mem::size_of::<libc::sockaddr_in6>())
                    .ok_or(SockAddrError::InsufficientBytes)?;
                // SAFETY: sockaddr_in6 can be cast from bytes as it is repr(C)
                let sockaddr_in6: &libc::sockaddr_in6 = unsafe {
                    sockaddr_in6_bytes
                        .align_to()
                        .1
                        .first()
                        .ok_or(SockAddrError::BadAlignment)?
                };

                let addr = u128::from_be_bytes(sockaddr_in6.sin6_addr.s6_addr);
                let port = u16::from_be(sockaddr_in6.sin6_port);
                let flowinfo = sockaddr_in6.sin6_flowinfo;
                let scope_id = sockaddr_in6.sin6_scope_id;

                Ok(SockAddr::Ipv6(SocketAddrV6::new(
                    Ipv6Addr::from_bits(addr),
                    port,
                    flowinfo,
                    scope_id,
                )))
            }
            libc::AF_UNIX => {
                let path_start = mem::offset_of!(libc::sockaddr_un, sun_path);
                match addr_bytes.get(path_start) {
                    Some(b'\0') => {
                        // abstract address
                        Ok(SockAddr::Unix(SocketAddrUnix::Abstract(
                            Buffer::from_slice(&addr_bytes[path_start + 1..]),
                        )))
                    }
                    Some(_) => {
                        // Named address
                        let path = CStr::from_bytes_until_nul(&addr_bytes[path_start..])
                            .map_err(|_| SockAddrError::MissingNullTerm)?;
                        Ok(SockAddr::Unix(SocketAddrUnix::Pathname(
                            FilePath::from_cstr(path)
                                .map_err(|_| SockAddrError::InvalidPathname)?,
                        )))
                    }
                    None if addr_bytes.len() == 2 => {
                        // Unnamed address
                        Ok(SockAddr::Unix(SocketAddrUnix::Unnamed))
                    }
                    _ => Err(SockAddrError::InsufficientBytes),
                }
            }
            _ => Err(SockAddrError::UnknownAddressFamily),
        }
    }

    pub fn encode(&self, addr_bytes: &mut [MaybeUninit<u8>]) -> usize {
        match self {
            SockAddr::Ipv4(v4_addr) => {
                let sockaddr_in = libc::sockaddr_in {
                    sin_family: libc::AF_INET as u16,
                    sin_port: v4_addr.port(),
                    sin_addr: libc::in_addr {
                        s_addr: u32::from_le_bytes(v4_addr.ip().octets()), // TODO: verify this is correct endianness
                    },
                    sin_zero: [0u8; 8],
                };

                // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_in to &[u8]
                let sockaddr_in_bytes: &[u8] =
                    unsafe { slice::from_ref(&sockaddr_in).align_to().1 };
                assert!(
                    sockaddr_in_bytes.len() == mem::size_of_val(&sockaddr_in),
                    "align_to() failed to convert sockaddr_in to bytes"
                );

                for (dst, src) in addr_bytes.iter_mut().zip(sockaddr_in_bytes) {
                    dst.write(*src);
                }

                mem::size_of_val(&sockaddr_in)
            }
            SockAddr::Ipv6(v6_addr) => {
                let sockaddr_in6 = libc::sockaddr_in6 {
                    sin6_family: libc::AF_INET6 as u16,
                    sin6_port: v6_addr.port(),
                    sin6_addr: libc::in6_addr {
                        s6_addr: v6_addr.ip().octets(), // TODO: verify this is correct endianness
                    },
                    sin6_flowinfo: v6_addr.flowinfo(),
                    sin6_scope_id: v6_addr.scope_id(),
                };

                // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_in6 to &[u8]
                let sockaddr_in6_bytes: &[u8] =
                    unsafe { slice::from_ref(&sockaddr_in6).align_to().1 };
                assert!(
                    sockaddr_in6_bytes.len() == mem::size_of_val(&sockaddr_in6),
                    "align_to() failed to convert sockaddr_in6 to bytes"
                );

                for (dst, src) in addr_bytes.iter_mut().zip(sockaddr_in6_bytes) {
                    dst.write(*src);
                }

                mem::size_of_val(&sockaddr_in6)
            }
            SockAddr::Unix(unix_addr) => match unix_addr {
                SocketAddrUnix::Abstract(unix_abstract) => {
                    let mut sockaddr_un = libc::sockaddr_un {
                        sun_family: libc::AF_UNIX as u16,
                        sun_path: [0i8; 108],
                    };

                    sockaddr_un.sun_path[0] = 0i8;
                    for (dst, src) in sockaddr_un.sun_path[1..]
                        .iter_mut()
                        .zip(unix_abstract.data())
                    {
                        *dst = *src as i8;
                    }

                    // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_un to &[u8]
                    let sockaddr_un_bytes: &[u8] =
                        unsafe { slice::from_ref(&sockaddr_un).align_to().1 };
                    assert!(
                        sockaddr_un_bytes.len() == mem::size_of_val(&sockaddr_un),
                        "align_to() failed to convert sockaddr_un to bytes"
                    );

                    let addrlen = mem::offset_of!(libc::sockaddr_un, sun_path)
                        + 1
                        + unix_abstract.data().len();

                    for (dst, src) in addr_bytes.iter_mut().zip(&sockaddr_un_bytes[..addrlen]) {
                        dst.write(*src);
                    }

                    addrlen
                }
                SocketAddrUnix::Pathname(unix_path) => {
                    let mut sockaddr_un = libc::sockaddr_un {
                        sun_family: libc::AF_UNIX as u16,
                        sun_path: [0i8; 108],
                    };

                    for (dst, src) in sockaddr_un.sun_path.iter_mut().zip(unix_path.data()) {
                        *dst = *src as i8;
                    }

                    // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_un to &[u8]
                    let sockaddr_un_bytes: &[u8] =
                        unsafe { slice::from_ref(&sockaddr_un).align_to().1 };
                    assert!(
                        sockaddr_un_bytes.len() == mem::size_of_val(&sockaddr_un),
                        "align_to() failed to convert sockaddr_un to bytes"
                    );

                    let addrlen =
                        mem::offset_of!(libc::sockaddr_un, sun_path) + unix_path.data().len();

                    for (dst, src) in addr_bytes.iter_mut().zip(&sockaddr_un_bytes[..addrlen]) {
                        dst.write(*src);
                    }

                    addrlen
                }
                SocketAddrUnix::Unnamed => {
                    let sun_family = libc::AF_UNIX as u16;

                    for (dst, src) in addr_bytes.iter_mut().zip(sun_family.to_be_bytes()) {
                        dst.write(src);
                    }

                    mem::size_of_val(&sun_family)
                }
            },
        }
    }

    pub fn encode_vec(&self, v: &mut Vec<u8>) {
        match self {
            SockAddr::Ipv4(v4_addr) => {
                let sockaddr_in = libc::sockaddr_in {
                    sin_family: libc::AF_INET as u16,
                    sin_port: v4_addr.port(),
                    sin_addr: libc::in_addr {
                        s_addr: u32::from_le_bytes(v4_addr.ip().octets()), // TODO: verify this is correct endianness
                    },
                    sin_zero: [0u8; 8],
                };

                // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_in to &[u8]
                let sockaddr_in_bytes: &[u8] =
                    unsafe { slice::from_ref(&sockaddr_in).align_to().1 };
                assert!(
                    sockaddr_in_bytes.len() == mem::size_of_val(&sockaddr_in),
                    "align_to() failed to convert sockaddr_in to bytes"
                );

                v.extend(sockaddr_in_bytes);
            }
            SockAddr::Ipv6(v6_addr) => {
                let sockaddr_in6 = libc::sockaddr_in6 {
                    sin6_family: libc::AF_INET6 as u16,
                    sin6_port: v6_addr.port(),
                    sin6_addr: libc::in6_addr {
                        s6_addr: v6_addr.ip().octets(), // TODO: verify this is correct endianness
                    },
                    sin6_flowinfo: v6_addr.flowinfo(),
                    sin6_scope_id: v6_addr.scope_id(),
                };

                // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_in to &[u8]
                let sockaddr_in6_bytes: &[u8] =
                    unsafe { slice::from_ref(&sockaddr_in6).align_to().1 };
                assert!(
                    sockaddr_in6_bytes.len() == mem::size_of_val(&sockaddr_in6),
                    "align_to() failed to convert sockaddr_in6 to bytes"
                );

                v.extend(sockaddr_in6_bytes);
            }
            SockAddr::Unix(unix_addr) => match unix_addr {
                SocketAddrUnix::Abstract(unix_abstract) => {
                    let mut sockaddr_un = libc::sockaddr_un {
                        sun_family: libc::AF_UNIX as u16,
                        sun_path: [0i8; 108],
                    };

                    sockaddr_un.sun_path[0] = 0i8;
                    for (dst, src) in sockaddr_un.sun_path[1..]
                        .iter_mut()
                        .zip(unix_abstract.data())
                    {
                        *dst = *src as i8;
                    }

                    // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_in to &[u8]
                    let sockaddr_un_bytes: &[u8] =
                        unsafe { slice::from_ref(&sockaddr_un).align_to().1 };
                    assert!(
                        sockaddr_un_bytes.len() == mem::size_of_val(&sockaddr_un),
                        "align_to() failed to convert sockaddr_un to bytes"
                    );

                    let addrlen = mem::offset_of!(libc::sockaddr_un, sun_path)
                        + 1
                        + unix_abstract.data().len();

                    v.extend(&sockaddr_un_bytes[..addrlen]);
                }
                SocketAddrUnix::Pathname(unix_path) => {
                    let mut sockaddr_un = libc::sockaddr_un {
                        sun_family: libc::AF_UNIX as u16,
                        sun_path: [0i8; 108],
                    };

                    for (dst, src) in sockaddr_un.sun_path.iter_mut().zip(unix_path.data()) {
                        *dst = *src as i8;
                    }

                    // SAFETY: u8 never should have alignment issues, so this should turn &sockaddr_in to &[u8]
                    let sockaddr_un_bytes: &[u8] =
                        unsafe { slice::from_ref(&sockaddr_un).align_to().1 };
                    assert!(
                        sockaddr_un_bytes.len() == mem::size_of_val(&sockaddr_un),
                        "align_to() failed to convert sockaddr_un to bytes"
                    );

                    let addrlen =
                        mem::offset_of!(libc::sockaddr_un, sun_path) + unix_path.data().len();

                    v.extend(&sockaddr_un_bytes[..addrlen]);
                }
                SocketAddrUnix::Unnamed => {
                    let sun_family = libc::AF_UNIX as u16;

                    v.extend(sun_family.to_be_bytes());
                }
            },
        }
    }
}

#[derive(Debug)]
pub enum SockAddrError {
    InsufficientBytes,
    BadAlignment,
    InvalidPathname,
    MissingNullTerm,
    UnknownAddressFamily,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TransportAddress {
    pub sockaddr: SockAddr,
    pub protocol: TransportProtocol,
}

impl Display for TransportAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", &self.protocol, &self.sockaddr)
    }
}

impl TransportAddress {
    pub fn new_inet(addr: SocketAddr, protocol: TransportProtocol) -> Self {
        assert_ne!(protocol, TransportProtocol::Unix);
        Self {
            sockaddr: match addr {
                SocketAddr::V4(v4_addr) => SockAddr::Ipv4(v4_addr),
                SocketAddr::V6(v6_addr) => SockAddr::Ipv6(v6_addr),
            },
            protocol,
        }
    }

    pub fn new_unix(addr: SocketAddrUnix) -> Self {
        Self {
            sockaddr: SockAddr::Unix(addr),
            protocol: TransportProtocol::Unix,
        }
    }

    pub fn protocol(&self) -> TransportProtocol {
        self.protocol
    }

    pub fn family(&self) -> AddressFamily {
        match &self.sockaddr {
            SockAddr::Ipv4(_) => AddressFamily::Ipv4,
            SockAddr::Ipv6(_) => AddressFamily::Ipv6,
            SockAddr::Unix(_) => AddressFamily::Unix,
        }
    }

    pub fn addr(&self) -> &SockAddr {
        &self.sockaddr
    }

    pub fn wildcard(&self) -> Option<TransportAddress> {
        match &self.sockaddr {
            SockAddr::Ipv4(v4_addr) => Some(TransportAddress {
                sockaddr: SockAddr::Ipv4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, v4_addr.port())),
                protocol: self.protocol,
            }),
            SockAddr::Ipv6(v6_addr) => Some(TransportAddress {
                sockaddr: SockAddr::Ipv6(SocketAddrV6::new(
                    Ipv6Addr::UNSPECIFIED,
                    v6_addr.port(),
                    v6_addr.flowinfo(),
                    v6_addr.scope_id(),
                )),
                protocol: self.protocol,
            }),
            SockAddr::Unix(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConfigTransportEndpoint {
    pub direction: SocketDirection,
    pub address: TransportAddress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SocketDirection {
    Client,
    Server,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransportProtocol {
    /// Transmission Control Protocol.
    Tcp,
    /// User Datagram Protocol.
    Udp,
    /// Stream Control Transmission Protocol.
    Sctp,
    /// Unix domain socket (e.g. no transport protocol).
    Unix,
}

impl Display for TransportProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sctp => f.write_str("SCTP"),
            Self::Tcp => f.write_str("TCP"),
            Self::Udp => f.write_str("UDP"),
            Self::Unix => f.write_str("Unix"),
        }
    }
}

impl TransportProtocol {
    pub fn raw(&self) -> i32 {
        match self {
            Self::Tcp => libc::IPPROTO_TCP,
            Self::Udp => libc::IPPROTO_UDP,
            Self::Sctp => libc::IPPROTO_SCTP,
            Self::Unix => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
    Unix,
    Netlink,
}

impl Display for AddressFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ipv4 => f.write_str("Ipv4"),
            Self::Ipv6 => f.write_str("Ipv6"),
            Self::Unix => f.write_str("Unix"),
            Self::Netlink => f.write_str("Netlink"),
        }
    }
}

impl AddressFamily {
    pub fn raw(&self) -> u16 {
        match self {
            Self::Ipv4 => libc::AF_INET as u16,
            Self::Ipv6 => libc::AF_INET6 as u16,
            Self::Unix => libc::AF_UNIX as u16,
            Self::Netlink => libc::AF_NETLINK as u16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SocketType {
    Stream,
    SeqPacket,
    Datagram,
    Raw,
}

impl Display for SocketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream => f.write_str("Stream"),
            Self::SeqPacket => f.write_str("SeqPacket"),
            Self::Datagram => f.write_str("Datagram"),
            Self::Raw => f.write_str("Raw"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IoSource {
    /// I/O emulating `stdin`/`stdout`.
    ///
    /// `stderr` is currently reserved for error messaging by fizzle.
    Stdio,
    /// I/O emulating a specific file location.
    File(FilePath<MAX_PATH_LEN>),
    /// I/O emulating a transport-layer socket.
    TransportSocket(ConfigTransportEndpoint),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SocketAddrUnix {
    Abstract(Buffer<MAX_UNIX_ABSTRACT_LEN>),
    Pathname(FilePath<MAX_UNIX_PATH_LEN>),
    Unnamed,
}

impl Display for SocketAddrUnix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pathname(path) => {
                write!(f, "{:?}", CStr::from_bytes_with_nul(path.data()).unwrap())
            }
            Self::Abstract(abs) => write!(f, "[{:?}]", abs.data()),
            Self::Unnamed => write!(f, "unnamed"),
        }
    }
}
