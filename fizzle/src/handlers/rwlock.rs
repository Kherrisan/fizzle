use std::collections::{HashSet, VecDeque};
use std::thread::ThreadId;

use fxhash::FxBuildHasher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RwLockPtr(usize);

impl From<*mut libc::pthread_rwlock_t> for RwLockPtr {
    fn from(value: *mut libc::pthread_rwlock_t) -> Self {
        RwLockPtr(value as usize)
    }
}

#[derive(Debug)]
pub struct RwLockInfo {
    pub state: RwLockState,
    pub awaiting_read: VecDeque<ThreadId>,
    pub awaiting_write: VecDeque<ThreadId>,
    pub holding_state: HashSet<ThreadId, FxBuildHasher>,
}

impl Default for RwLockInfo {
    fn default() -> Self {
        Self {
            state: RwLockState::Available,
            awaiting_read: VecDeque::new(),
            awaiting_write: VecDeque::new(),
            holding_state: HashSet::with_hasher(Default::default()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RwLockState {
    Available,
    Reading,
    Writing,
}
