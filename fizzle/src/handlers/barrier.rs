use std::thread::ThreadId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BarrierPtr(usize);

impl From<*mut libc::pthread_barrier_t> for BarrierPtr {
    fn from(value: *mut libc::pthread_barrier_t) -> Self {
        BarrierPtr(value as usize)
    }
}

#[derive(Debug)]
pub struct BarrierInfo {
    pub curr: Vec<ThreadId>,
    pub needed: usize,
}
