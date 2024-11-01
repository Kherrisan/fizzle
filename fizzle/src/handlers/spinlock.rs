#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpinlockPtr(usize);

impl From<*mut libc::pthread_spinlock_t> for SpinlockPtr {
    fn from(value: *mut libc::pthread_spinlock_t) -> Self {
        SpinlockPtr(value as usize)
    }
}
