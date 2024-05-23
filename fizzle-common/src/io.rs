
// `SocketAddr` does not use heap allocations, so it's safe for this type.
use std::os::unix::net::SocketAddr;

use crate::path::FilePath;

pub enum IoLocation {
    /// Treats `stdin`/`stdout` as an I/O location.
    /// 
    /// `stderr` is currently reserved for error messaging by fizzle.
    Stdio,
    File(FilePath),
    TcpSocket(SocketAddr),
    UdpSocket(SocketAddr),
    SctpSocket(SocketAddr),
}
