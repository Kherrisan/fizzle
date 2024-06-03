use std::fmt::Debug;

use crate::state::identifiers::*;

use self::private::Sealed;

mod private {
    #[derive(Clone, Copy, Debug)]
    pub struct Sealed;
}

#[derive(Clone, Copy, Debug)]
pub enum IoBackend<R: Clone + Copy + Debug, F: Clone + Copy + Debug, P: Clone + Copy + Debug> {
    Passthrough,
    /// Handles I/O regularly.
    Regular(R),
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual file.
    // TODO: need to turn this into FeedbackInfo complete with `Polled` for read/write
    Feedback(F),
    /// Uses the plugin specified by `PluginId` to decide `read()`/`write()` behavior.
    Plugin(P),
    Sink,
    NullSink,
    /// Indicates that fuzzing input should be passed directly through the I/O Endpoint.
    /// 
    /// The `usize` value specifies the index of fuzzed input that has been read to.
    #[allow(unused)]
    Fuzz(usize),
    // TODO: add Passthrough here?
}

#[derive(Clone, Copy, Debug)]
pub struct StandardFeedback {
    pub buf: BufferId,
    pub read_polled: PolledId,
    pub write_polled: PolledId,
}

#[derive(Clone, Copy, Debug)]
pub struct StandardPlugin {
    pub plugin_id: PluginId,
    pub read_buf: BufferId,
    pub read_polled: PolledId,
    pub write_buf: BufferId,
    pub write_polled: PolledId
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


pub type PendingBackend = IoBackend<(), (), PluginId>;

pub type ConnectingBackend = IoBackend<(), (), PluginId>;

/// The backend for a connected socket.
pub type ConnectedBackend = IoBackend<RegularConnected, StandardFeedback, StandardPlugin>;

/// The backend for a connectionless socket.
pub type ConnectionlessBackend = IoBackend<RegularConnectionless, StandardFeedback, StandardPlugin>;

pub type FileBackend = IoBackend<Sealed, StandardFeedback, StandardPlugin>;

pub type ServerBackend = IoBackend<(), (), PluginId>;

pub type StdioBackend = IoBackend<Sealed, StandardFeedback, StandardPlugin>;


