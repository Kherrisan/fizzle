use crate::arena::{ArenaKey, Rc}; 

pub use private::EventfdId;

use super::polled::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct EventfdId(usize);
}

#[derive(Clone, Debug)]
pub struct EventfdInfo {
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
    pub is_semaphore: bool,
    pub counter: u64,
}

impl ArenaKey for EventfdId {
    type Value = EventfdInfo;
}

impl EventfdId {

}
