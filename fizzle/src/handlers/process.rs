use std::mem::MaybeUninit;
use std::ptr;

use crate::arena::ArenaKey;
use crate::semaphore::Semaphore;
use crate::state::FizzleSingleton;

use super::signal::ProcSigInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId(usize);

impl From<ProcessId> for usize {
    fn from(value: ProcessId) -> Self {
        value.0
    }
}

impl From<usize> for ProcessId {
    fn from(value: usize) -> Self {
        ProcessId(value)
    }
}

impl ArenaKey for ProcessId {
    type Value = ProcSigInfo;
}

impl ProcessId {
    pub fn main_process() -> Self {
        Self(0)
    }

    pub fn is_main_process(&self) -> bool {
        self.0 == 0
    }
}
