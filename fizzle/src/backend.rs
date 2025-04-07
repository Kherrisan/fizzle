use std::collections::LinkedList;

use crate::handlers::fuzz_endpoint::FuzzEndpointInfo;
use crate::handlers::plugin::PluginInfo;
use crate::handlers::polled::PolledInfo;
use crate::handlers::socket::{ConnectionlessMessage, SocketInfo};
use crate::{GlobalDeque, GlobalHeap, GlobalRc, GlobalVec, GlobalWeak};

use self::private::Sealed;

mod private {
    /// Indicates that the given [`IoBackend`](super::IoBackend) should not be constructed.
    #[derive(Clone, Copy, Debug)]
    pub struct Sealed;
}

#[derive(Clone)]
pub enum IoBackend<R: Clone, F: Clone> {
    Passthrough,
    /// Handles I/O regularly.
    Peered(R),
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual FIFO queue.
    Feedback(F),
    /// Uses the plugin specified by `PluginEndpointId` to decide `read()`/`write()` behavior.
    Plugin(GlobalRc<PluginInfo>),
    Sink,
    NullSink,
    /// Indicates that fuzzing input should be passed directly through the I/O Endpoint.
    ///
    /// The `usize` value specifies the index of fuzzed input that has been read to.
    Fuzz(GlobalRc<FuzzEndpointInfo>),
}

#[derive(Clone)]
pub struct StandardFeedback {
    pub buf: LinkedList<GlobalVec<u8>, GlobalHeap>,
    pub read_idx: usize,
    pub read_polled: GlobalRc<PolledInfo>,
    pub write_polled: GlobalRc<PolledInfo>,
}

#[derive(Clone)]
pub struct FileFeedback {}

#[derive(Clone)]
pub struct RegularConnected {
    pub peer: GlobalWeak<SocketInfo>,
    pub recv_buf: LinkedList<GlobalVec<u8>, GlobalHeap>,
    pub read_idx: usize,
    pub read_polled: GlobalRc<PolledInfo>,
    pub write_polled: GlobalRc<PolledInfo>,
}

#[derive(Clone)]
pub struct RegularConnectionless {
    pub recv_buf: LinkedList<ConnectionlessMessage, GlobalHeap>,
    pub read_polled: GlobalRc<PolledInfo>,
    pub write_polled: GlobalRc<PolledInfo>,
}

#[derive(Clone)]
pub struct FeedbackConnectionless {
    pub feedback_buf: GlobalDeque<(ConnectionlessMessage, GlobalRc<SocketInfo>)>,
}

/// A backend for a Pending socket connection.
pub type PendingBackend = IoBackend<(), ()>;

/// A backend for a socket client that is actively connecting to a server.
pub type ConnectingBackend = IoBackend<(), ()>;

/// The backend for a connected socket.
pub type ConnectedBackend = IoBackend<RegularConnected, StandardFeedback>;

/// The backend for a connectionless (UDP) socket.
pub type ConnectionlessBackend = IoBackend<RegularConnectionless, FeedbackConnectionless>;

/// A backend for a file handle.
pub type FileBackend = IoBackend<Sealed, FileFeedback>;

/// A backend for a server socket.
pub type ServerBackend = IoBackend<(), ()>;

/// A backend for `stdin`/`stdout`.
pub type StdioBackend = IoBackend<Sealed, StandardFeedback>;
