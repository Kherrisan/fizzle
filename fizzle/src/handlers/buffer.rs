use crate::constants::FIZZLE_BUFFER_LENGTH;

use fizzle_common::storage::Buffer;

pub use private::BufferId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct BufferId(usize);
}
