use crate::arena::{ArenaKey, Rc};
use crate::constants::FIZZLE_MAX_PER_EVENT_QUEUED_POLLERS;
use crate::state::{FizzleSingleton, ReadyInfo};

use super::poller::PollerId;

pub use private::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PolledId(usize);
}

// Each time a Polled is *raised* (i.e., goes from `event_raised: false` to `event_raised: true`),
// the PolledInfo will move all of its `pollers` into the ready queue (if they are not already there).
#[derive(Debug, Default)]
pub struct PolledInfo {
    /// Pollers that this Polled instance is meant to awaken
    pub pollers: heapless::Vec<Rc<PollerId>, FIZZLE_MAX_PER_EVENT_QUEUED_POLLERS>,
    /// Indicates that the item being polled is "ready" for the `Poller`.
    pub event_raised: bool,
    // /// Indicates that a `Poller` has been sent to the ready queue from this `Polled` instance and
    // /// has not yet been executed.
    // pub poller_dispatched: bool,
}

impl PolledInfo {
    pub fn new() -> Self {
        Self {
            pollers: heapless::Vec::new(),
            event_raised: false,
        }
    }

    pub fn new_raised() -> Self {
        Self {
            pollers: heapless::Vec::new(),
            event_raised: true,
        }
    }
}

impl ArenaKey for PolledId {
    type Value = PolledInfo;
}
