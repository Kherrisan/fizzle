use std::fmt::Debug;

use crate::arena::Rc;
use crate::handlers::buffer::BufferId;
use crate::handlers::fuzz_endpoint::FuzzEndpointId;
use crate::handlers::plugin::PluginEndpointId;
use crate::handlers::polled::PolledId;
use crate::handlers::socket::SocketId;

use self::private::Sealed;

mod private {
    /// Indicates that the given [`IoBackend`](super::IoBackend) should not be constructed.
    #[derive(Clone, Copy, Debug)]
    pub struct Sealed;
}

#[derive(Clone, Debug)]
pub enum IoBackend<R: Clone + Debug, F: Clone + Debug> {
    Passthrough,
    /// Handles I/O regularly.
    Peered(R),
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual FIFO queue.
    Feedback(F),
    /// Uses the plugin specified by `PluginEndpointId` to decide `read()`/`write()` behavior.
    Plugin(Rc<PluginEndpointId>),
    Sink,
    NullSink,
    /// Indicates that fuzzing input should be passed directly through the I/O Endpoint.
    ///
    /// The `usize` value specifies the index of fuzzed input that has been read to.
    Fuzz(Rc<FuzzEndpointId>),
}

#[derive(Clone, Debug)]
pub struct StandardFeedback {
    pub buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
}

#[derive(Clone, Debug)]
pub struct FileFeedback { }

#[derive(Clone, Debug)]
pub struct RegularConnected {
    pub peer: Option<Rc<SocketId>>,
    pub recv_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
}

#[derive(Clone, Debug)]
pub struct RegularConnectionless {
    pub recv_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
}

/// A backend for a Pending socket connection.
pub type PendingBackend = IoBackend<(), ()>;

/// A backend for a socket client that is actively connecting to a server.
pub type ConnectingBackend = IoBackend<(), ()>;

/// The backend for a connected socket.
pub type ConnectedBackend = IoBackend<RegularConnected, StandardFeedback>;

/// The backend for a connectionless (UDP) socket.
pub type ConnectionlessBackend = IoBackend<RegularConnectionless, StandardFeedback>;

/// A backend for a file handle.
pub type FileBackend = IoBackend<Sealed, FileFeedback>;

/// A backend for a server socket.
pub type ServerBackend = IoBackend<(), ()>;

/// A backend for `stdin`/`stdout`.
pub type StdioBackend = IoBackend<Sealed, StandardFeedback>;
