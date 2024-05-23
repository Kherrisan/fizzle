
// `SocketAddr` does not use heap allocations, so it's safe for this type.
use std::net::SocketAddr;

use crate::path::FilePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SocketLocation {
    pub socket_type: SocketType,
    pub socket_addr: SocketAddr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SocketType {
    /// A Transmission Control Protocol client.
    TcpClient,
    /// A Transmission Control Protocol server.
    TcpServer,
    /// A User Datagram Protocol client.
    UdpClient,
    /// A User Datagram Protocol server.
    UdpServer,
    /// A Stream Control Transmission Protocol client.
    SctpClient,
    /// A Stream Control Transmission Protocol server.
    SctpServer,
}

pub enum IoLocation {
    /// I/O emulating `stdin`/`stdout`.
    /// 
    /// `stderr` is currently reserved for error messaging by fizzle.
    Stdio,
    /// I/O emulating a specific file location.
    File(FilePath),
    /// I/O emulating a transport-layer socket.
    TransportSocket(SocketLocation),
}
