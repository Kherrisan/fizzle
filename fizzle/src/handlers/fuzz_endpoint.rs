use crate::arena::{ArenaKey, Rc};

pub use private::FuzzEndpointId;

use super::polled::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FuzzEndpointId(usize);
}

#[derive(Clone, Debug)]
pub struct FuzzEndpointInfo {
    pub read_polled: Rc<PolledId>,
    pub read_idx: usize,
}

impl ArenaKey for FuzzEndpointId {
    type Value = FuzzEndpointInfo;
}

impl FuzzEndpointId {

}


