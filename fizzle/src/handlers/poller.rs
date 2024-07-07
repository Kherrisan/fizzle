use crate::arena::{ArenaKey, Rc};
use crate::constants::FIZZLE_MAX_PER_POLLER_QUEUED_EVENTS;
use crate::state::WorkerId;

pub use private::PollerId;

use super::polled::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PollerId(usize);
}

// Each time a Polled is *raised* (i.e., goes from `event_raised: false` to `event_raised: true`),
// the PolledInfo will move all of its `pollers` into the ready queue (if they are not already there).
#[derive(Debug)]
pub struct PollerInfo {
    pub worker_id: WorkerId,
    pub polled_events: heapless::Vec<Rc<PolledId>, FIZZLE_MAX_PER_POLLER_QUEUED_EVENTS>,
    pub in_raised_queue: bool,
}

impl ArenaKey for PollerId {
    type Value = PollerInfo;
}

impl PollerId {

}
