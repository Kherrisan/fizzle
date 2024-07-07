use crate::arena::{ArenaKey, Rc};
use crate::backend::{ConnectedBackend, ConnectingBackend, ConnectionlessBackend, PendingBackend, ServerBackend};
use crate::constants::FIZZLE_SOMAXCONN;

use fizzle_common::io::{AddressFamily, SocketType, TransportAddress, TransportProtocol};
pub use private::SocketId;

use super::polled::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SocketId(usize);
}

#[derive(Debug)]
pub struct SocketLocationInfo {
    /// The socket bound to the given location.
    pub bound_socket: Option<Rc<SocketId>>,
    /// Points to an optional linked list of clients that are awaiting this location to exist.
    pub pending: Option<PendingInfo>,
}

#[derive(Clone, Debug)]
pub struct PendingInfo {
    pub client: Rc<SocketId>,
    pub poll: Rc<PolledId>,
}

#[derive(Debug)]
pub enum SocketState {
    Connectionless(ConnectionlessSocket),
    Unassociated(UnassociatedSocket),
    Server(ServerSocket),
    PendingConnection(PendingSocket),
    Connecting(ConnectingSocket),
    Connected(ConnectedSocket),
    //    Error state?
}

#[derive(Debug)]
pub struct ConnectionlessSocket {
    pub backend: ConnectionlessBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: Option<TransportAddress>,
}

#[derive(Debug)]
pub struct UnassociatedSocket {
    pub local_addr: Option<TransportAddress>,
    pub family: AddressFamily,
    pub protocol: TransportProtocol,
    pub socktype: SocketType,
}

#[derive(Debug)]
pub struct ServerSocket {
    pub backend: ServerBackend,
    pub local_addr: TransportAddress,
    pub connecting: heapless::spsc::Queue<Rc<SocketId>, FIZZLE_SOMAXCONN>,
    pub ready_to_connect: Rc<PolledId>,
}

#[derive(Clone, Debug)]
pub struct PendingSocket {
    pub backend: PendingBackend,
    pub next_pending: Option<Rc<SocketId>>,
    pub src_addr: TransportAddress,
    pub rem_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectingSocket {
    pub backend: ConnectingBackend,
    pub connect_polled: Rc<PolledId>,
    pub local_addr: TransportAddress,
}

#[derive(Debug)]
pub struct ConnectedSocket {
    pub backend: ConnectedBackend,
    pub local_addr: TransportAddress,
    pub rem_addr: TransportAddress,
    pub peer_closed: bool,
}

impl ArenaKey for SocketId {
    type Value = SocketState;
}

impl SocketId {

}
