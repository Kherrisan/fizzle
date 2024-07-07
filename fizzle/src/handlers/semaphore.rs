use crate::arena::ArenaKey; 
use crate::constants::FIZZLE_MAX_WAITING_SEMAPHORES;
use crate::state::WorkerId;

pub use private::SemaphoreId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SemaphoreId(usize);
}


impl ArenaKey for SemaphoreId {
    type Value = SemaphoreInfo;
}

#[derive(Debug)]
pub struct SemaphoreInfo {
    pub refs: usize,
    pub unlinked: bool,
    pub value: usize,
    pub waiting: heapless::Deque<WorkerId, FIZZLE_MAX_WAITING_SEMAPHORES>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemaphorePtr(usize);

impl From<*mut libc::sem_t> for SemaphorePtr {
    fn from(value: *mut libc::sem_t) -> Self {
        SemaphorePtr(value as usize)
    }
}

impl SemaphorePtr {
    pub fn to_mut_ptr(self) -> *mut libc::sem_t {
        self.0 as *mut libc::sem_t
    }
}

impl SemaphoreId {

}

