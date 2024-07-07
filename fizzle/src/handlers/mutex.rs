#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutexPtr(usize);

impl From<*mut libc::pthread_mutex_t> for MutexPtr {
    fn from(value: *mut libc::pthread_mutex_t) -> Self {
        MutexPtr(value as usize)
    }
}
