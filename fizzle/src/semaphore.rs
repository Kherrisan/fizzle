use std::cell::UnsafeCell;
use std::mem;

/// A wrapper for a POSIX semaphore, suitable for use in inter-process shared memory.
/// 
/// This API is "leaky" by default; `Semaphore`s that go out of scope will not be destroyed.
/// This is to accomadate shared memory, as multiple processes may claim "ownership" of the same `Semaphore`.
/// To clean up a semaphore, use [`destroy()`](Semaphore::destroy).
pub struct Semaphore {
    inner: UnsafeCell<libc::sem_t>,
}

impl Semaphore {
    pub fn new(value: u16) -> Self {
        let sem = Self {
            inner: UnsafeCell::new(unsafe { mem::zeroed() })
        };

        let res = unsafe { libc::sem_init(sem.inner.get(), libc::PTHREAD_PROCESS_SHARED, value as u32) };
        if res != 0 {
            panic!("platform does not support process-shared semaphores");
        }

        sem
    }

    pub fn wait(&self) {
        loop {
            let res = unsafe { libc::sem_wait(self.inner.get()) };
            if res == 0 {
                break
            } else if unsafe { *libc::__errno_location() } != libc::EINTR {
                panic!("semaphore internal error during wait()");
            }
        }
    }

    pub fn post(&self) {
        let res = unsafe { libc::sem_post(self.inner.get()) };
        if res != 0 {
            panic!("semaphore internal error during post()");
        }
    }

    /// Deallocates the POSIX semaphore.
    /// 
    /// # Safety
    /// 
    /// This method must only be called once on a given semaphore. Multiple processes calling
    /// `destroy()` on the same semaphore may result in undefined behavior.
    /// 
    /// This method must only be called once no other processes or threads are waiting on the
    /// semaphore (see [wait()](Semaphore::wait)); destroying a semaphore that is being waited on
    /// may result in undefined behavior.
    pub unsafe fn destroy(self) {
        let res = unsafe { libc::sem_destroy(self.inner.get()) };
        if res != 0 {
            panic!("semaphore internal error during destroy()");
        }
    }
}

/*
impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe { libc::sem_destroy(self.inner.get()) };
    }
}
*/


