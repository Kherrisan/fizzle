use crate::arena::ArenaKey;

pub use private::MessageQueueId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct MessageQueueId(usize);
}

#[derive(Debug)]
pub struct MessageQueueInfo {}

impl ArenaKey for MessageQueueId {
    type Value = MessageQueueInfo;
}

impl MessageQueueId {}
