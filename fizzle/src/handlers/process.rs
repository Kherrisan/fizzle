use std::mem::MaybeUninit;
use std::ptr;

use crate::arena::ArenaKey;
use crate::semaphore::Semaphore;
use crate::state::{FizzleSingleton, SignalInfo};

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
    type Value = SignalInfo;
}

impl ProcessId {
    pub fn is_main_process(&self) -> bool {
        self.0 == 0
    }

    pub fn init_process_lock(&self, ctx: &mut FizzleSingleton) {
        let sem_opt = &mut ctx.acquire().global.process_locks[usize::from(*self)];

        // TODO: this seems to behave safely, but check with Miri
        unsafe {
            let uninit_sem = (*(ptr::from_mut(sem_opt) as *mut Option<MaybeUninit<Semaphore>>)).insert(MaybeUninit::uninit());
            Semaphore::initialize(uninit_sem, false, 0);
        }
    }
}
