// `SocketAddr` does not use heap allocations, so it's safe for this type.
use std::net::SocketAddr;

use crate::path::FilePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransportEndpoint {
    pub direction: SocketDirection,
    pub transport_addr: TransportAddress,
}

// TODO: rename this
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransportAddress {
    Tcp(SocketAddr),
    Udp(SocketAddr),
    Sctp(SocketAddr),
}

impl TransportAddress {
    pub fn protocol(&self) -> TransportProtocol {
        match self {
            Self::Tcp(_) => TransportProtocol::Tcp,
            Self::Udp(_) => TransportProtocol::Udp,
            Self::Sctp(_) => TransportProtocol::Sctp,
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
        }
    }

    pub fn address(&self) -> &SocketAddr {
        match self {
            Self::Tcp(addr) => addr,
            Self::Udp(addr) => addr,
            Self::Sctp(addr) => addr,
        }
    }
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IoSource {
    /// I/O emulating `stdin`/`stdout`.
    ///
    /// `stderr` is currently reserved for error messaging by fizzle.
    Stdio,
    /// I/O emulating a specific file location.
    File(FilePath),
    /// I/O emulating a transport-layer socket.
    TransportSocket(TransportEndpoint),
}
