use std::ptr;

use crate::{GlobalRc, GlobalVec};
use super::poller::PollerInfo;

// Each time a Polled is *raised* (i.e., goes from `event_raised: false` to `event_raised: true`),
// the PolledInfo will move all of its `pollers` into the ready queue (if they are not already there).
pub struct PolledInfo {
    /// Pollers that this Polled instance is meant to awaken
    pub pollers: GlobalVec<GlobalRc<PollerInfo>>,
    /// Indicates that the item being polled is "ready" for the `Poller`.
    pub event_raised: bool,
    // /// Indicates that a `Poller` has been sent to the ready queue from this `Polled` instance and
    // /// has not yet been executed.
    // pub poller_dispatched: bool,
}

impl PartialEq for PolledInfo {
    fn eq(&self, other: &Self) -> bool {
        // Allows for an Rc<PolledInfo> to be equal when needed
        ptr::from_ref(self) == ptr::from_ref(other)
    }
}

impl Eq for PolledInfo {}

impl PartialOrd for PolledInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PolledInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        ptr::from_ref(self).cmp(&ptr::from_ref(other))
    }
}
