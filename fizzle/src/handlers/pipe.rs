use crate::arena::{ArenaKey, Rc};

pub use private::PipeId;

use super::buffer::BufferId;
use super::polled::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PipeId(usize);
}

#[derive(Debug)]
pub struct PipeInfo {
    /// The transmission mode of the packet.
    ///
    /// See [`PipeMode`] for more details.
    pub mode: PipeMode,
    /// The peer pipe that this pipe is connected to.
    ///
    /// If this value is `None`, then the pipe has broken (e.g., the other end has shut).
    pub peer: Option<Rc<PipeId>>,
    /// The buffer this pipe reads in data from.
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
}

/// The mode of operation by which data is passed over the pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeMode {
    /// Performs I/O in "packet" mode--writes are treated as individual packets.
    Direct,
    /// Performs I/O as if data is a constant stream.
    Streamed,
}

impl ArenaKey for PipeId {
    type Value = PipeInfo;
}

impl PipeId {

}

