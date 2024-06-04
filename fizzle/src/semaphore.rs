use std::mem::MaybeUninit;

/// A wrapper for a POSIX semaphore.
pub struct Semaphore {
    inner: Box<MaybeUninit<libc::sem_t>>,
}

impl Semaphore {
    pub fn new(value: u16) -> Self {
        let mut sem = Self {
            inner: Box::new(MaybeUninit::uninit()),
        };

        let res = unsafe {
            libc::sem_init(
                sem.inner.as_mut_ptr() as *mut libc::sem_t,
                libc::PTHREAD_PROCESS_PRIVATE,
                value as u32,
            )
        };
        if res != 0 {
            panic!("platform does not support process-shared semaphores");
        }

        sem
    }

    pub fn wait(&mut self) {
        loop {
            let res = unsafe { libc::sem_wait(self.inner.as_mut_ptr() as *mut libc::sem_t) };
            if res == 0 {
                break;
            } else if unsafe { *libc::__errno_location() } != libc::EINTR {
                panic!("semaphore internal error during wait()");
            }
        }
    }

    pub fn post(&mut self) {
        let res = unsafe { libc::sem_post(self.inner.as_mut_ptr() as *mut libc::sem_t) };
        if res != 0 {
            panic!("semaphore internal error during post()");
        }
    }
}


impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe { libc::sem_destroy(self.inner.as_mut_ptr() as *mut libc::sem_t) };
    }
}

