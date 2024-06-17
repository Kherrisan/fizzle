// `SocketAddr` does not use heap allocations, so it's safe for this type.
use std::{fmt::Display, net::SocketAddr};

use crate::{path::FilePath, storage::Buffer};

pub const MAX_PATH_LEN: usize = 256;
pub const MAX_UNIX_ABSTRACT_LEN: usize = 107;
pub const MAX_UNIX_PATH_LEN: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TransportEndpoint {
    pub direction: SocketDirection,
    pub transport_addr: TransportAddress,
}

// TODO: rename this
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TransportAddress {
    Sctp(SocketAddr),
    Tcp(SocketAddr),
    Udp(SocketAddr),
    Unix(UnixAddr),
}

impl TransportAddress {
    pub fn new_internet(addr: SocketAddr, protocol: TransportProtocol) -> Self {
        match protocol {
            TransportProtocol::Tcp => TransportAddress::Tcp(addr),
            TransportProtocol::Udp => TransportAddress::Udp(addr),
            TransportProtocol::Sctp => TransportAddress::Sctp(addr),
            TransportProtocol::Unix => unreachable!(),
        }
    }
    
    pub fn protocol(&self) -> TransportProtocol {
        match self {
            Self::Tcp(_) => TransportProtocol::Tcp,
            Self::Udp(_) => TransportProtocol::Udp,
            Self::Sctp(_) => TransportProtocol::Sctp,
            Self::Unix(_) => TransportProtocol::Unix,
        }
    }

    pub fn family(&self) -> AddressFamily {
        match self {
            Self::Tcp(SocketAddr::V4(_)) => AddressFamily::Ipv4,
            Self::Tcp(SocketAddr::V6(_)) => AddressFamily::Ipv6,
            Self::Udp(SocketAddr::V4(_)) => AddressFamily::Ipv4,
            Self::Udp(SocketAddr::V6(_)) => AddressFamily::Ipv6,
            Self::Sctp(SocketAddr::V4(_)) => AddressFamily::Ipv4,
            Self::Sctp(SocketAddr::V6(_)) => AddressFamily::Ipv6,
            Self::Unix(_) => AddressFamily::Unix,
        }
    }

    /*
    pub fn address(&self) -> &SocketAddr {
        match self {
            Self::Tcp(addr) => addr,
            Self::Udp(addr) => addr,
            Self::Sctp(addr) => addr,
        }
    }
    */
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
            Self::Sctp => f.write_str("Sctp"),
            Self::Tcp => f.write_str("Tcp"),
            Self::Udp => f.write_str("Udp"),
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
}

impl Display for AddressFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ipv4 => f.write_str("Ipv4"),
            Self::Ipv6 => f.write_str("Ipv6"),
            Self::Unix => f.write_str("Unix"),
        }
    }
}

impl AddressFamily {
    pub fn raw(&self) -> i32 {
        match self {
            Self::Ipv4 => libc::AF_INET,
            Self::Ipv6 => libc::AF_INET6,
            Self::Unix => libc::AF_UNIX,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SocketType {
    Stream,
    SeqPacket,
    Datagram,
}

impl Display for SocketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream => f.write_str("Stream"),
            Self::SeqPacket => f.write_str("SeqPacket"),
            Self::Datagram => f.write_str("Datagram"),
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
    TransportSocket(TransportEndpoint),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnixAddr {
    Abstract(Buffer<MAX_UNIX_ABSTRACT_LEN>),
    Pathname(FilePath<MAX_UNIX_PATH_LEN>),
    Unnamed,
}
