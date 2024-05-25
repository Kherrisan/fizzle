
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
pub struct TransportAddress {
    pub protocol: TransportProtocol,
    pub sockaddr: SocketAddr,
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IoEndpoint {
    /// I/O emulating `stdin`/`stdout`.
    /// 
    /// `stderr` is currently reserved for error messaging by fizzle.
    Stdio,
    /// I/O emulating a specific file location.
    File(FilePath),
    /// I/O emulating a transport-layer socket.
    TransportSocket(TransportEndpoint),
}
