use std::fmt::Debug;

use crate::state::identifiers::*;

use self::private::Sealed;

mod private {
    /// Indicates that the given [`IoBackend`](super::IoBackend) should not be constructed.
    #[derive(Clone, Copy, Debug)]
    pub struct Sealed;
}

#[derive(Clone, Copy, Debug)]
pub enum IoBackend<R: Clone + Copy + Debug, F: Clone + Copy + Debug> {
    Passthrough,
    /// Handles I/O regularly.
    Regular(R),
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual FIFO queue.
    Feedback(F),
    /// Uses the plugin specified by `PluginId` to decide `read()`/`write()` behavior.
    Plugin(PluginId),
    Sink,
    NullSink,
    /// Indicates that fuzzing input should be passed directly through the I/O Endpoint.
    /// 
    /// The `usize` value specifies the index of fuzzed input that has been read to.
    #[allow(unused)]
    Fuzz(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct StandardFeedback {
    pub buf: BufferId,
    pub read_polled: PolledId,
    pub write_polled: PolledId,
}

#[derive(Clone, Copy, Debug)]
pub struct RegularConnected {
    pub peer: Option<SocketId>,
    pub recv_buf: BufferId,
    pub read_polled: PolledId,
    pub write_polled: PolledId,
}

#[derive(Clone, Copy, Debug)]
pub struct RegularConnectionless {
    pub recv_buf: BufferId,
    pub read_polled: PolledId,
    pub write_polled: PolledId,
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
pub type FileBackend = IoBackend<Sealed, StandardFeedback>;

/// A backend for a server socket.
pub type ServerBackend = IoBackend<(), ()>;

/// A backend for `stdin`/`stdout`.
pub type StdioBackend = IoBackend<Sealed, StandardFeedback>;
