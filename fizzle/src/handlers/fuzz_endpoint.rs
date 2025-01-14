use crate::GlobalRc;

pub use private::FuzzEndpointId;

use super::polled::PolledInfo;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FuzzEndpointId(usize);
}

#[derive(Clone)]
pub struct FuzzEndpointInfo {
    pub read_polled: GlobalRc<PolledInfo>,
    pub read_idx: usize,
}

impl FuzzEndpointId {}
