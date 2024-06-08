use std::ptr;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;

unsafe fn raw(s: &Semaphore) -> *mut libc::sem_t {
    // if let Some(init) = &s.initialized {
    //     if !init.load(Ordering::Relaxed) {
    //         init_semaphore(s)
    //     }
    // }
    s.inner.get().cast()
}

/*
#[cold]
#[inline(never)]
unsafe fn init_semaphore(s: &Semaphore) {
    s.initialized.as_ref().unwrap().store(true, Ordering::Relaxed);
    let sem = s.inner.get().cast();
    assert!(libc::sem_init(sem, libc::PTHREAD_PROCESS_PRIVATE, 0) == 0);
}
*/

pub struct Semaphore {
    // initialized: Option<AtomicBool>,
    inner: UnsafeCell<MaybeUninit<libc::sem_t>>,
}

impl Semaphore {

    
    /*
    /// Constructs a new semaphore in place.
    /// 
    /// This method MUST ONLY be used in global contexts (e.g. for a static variable); see `Safety` 
    /// for more details.
    /// 
    /// # Safety
    /// 
    /// This method lazily initializes the semaphore in a thread-unsafe way. It should not be used in
    /// multithreaded contexts unless guarantees are put in place that only one thread is executing at
    /// the time this method is called.
    /// 
    /// `Sem` is not safe to relocate in memory; once allocated, it must remain at a fixed address.
    /// Any movement of `Sem` (even into a Box via (`Box::new(Sem::static_new())`) will lead to
    /// undefined behavior. It is up to the caller to ensure that this method is only called in a
    /// static context.
    pub const fn static_new() -> Semaphore {
        Semaphore {
            initialized: Some(AtomicBool::new(false)),
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
    */

    /*
    pub fn new_option(value: u32) -> Option<Semaphore> {
        let mut s = Some(Semaphore {
            // initialized: None,
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        });

        unsafe {
            let sem = s.as_mut().unwrap().inner.get().cast();
            assert!(libc::sem_init(sem, libc::PTHREAD_PROCESS_PRIVATE, value) == 0);
        }
        
        s
    }
    */

    /// Constructs a new semaphore within a boxed memory region.
    /// 
    /// This method is safe to use under
    /// most normal circumstances, but the enclosed `Sem` must not be moved out of the box or
    /// undefined behavior will occur.
    pub fn new_boxed(value: u32) -> Box<Semaphore> {
        let s = Box::new(Semaphore {
            // initialized: None,
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        });

        unsafe {
            let sem = s.inner.get().cast();
            assert!(libc::sem_init(sem, libc::PTHREAD_PROCESS_PRIVATE, value) == 0);
        }

        s
    }

    /// Initializes a semaphore in-place.
    /// 
    /// This method is safe to use with shared memory to enable inter-process communication locks.
    pub fn initialize(sem: &mut MaybeUninit<Semaphore>, shared: bool, value: u32) -> &mut Semaphore {
        let access = match shared {
            true => libc::PTHREAD_PROCESS_SHARED,
            false => libc::PTHREAD_PROCESS_PRIVATE,
        };

        unsafe {
            // ptr::addr_of_mut!((*sem.as_mut_ptr()).initialized).write(None);
            ptr::addr_of_mut!((*sem.as_mut_ptr()).inner).write(UnsafeCell::new(MaybeUninit::uninit()));
            // Safety: the memory of `sem` is now all initialized
            let init_sem = sem.assume_init_mut();
            assert!(libc::sem_init(init_sem.inner.get().cast(), access, value) == 0);
            init_sem
        }
    }

    /*
    pub fn try_wait(&self) {
        unsafe {
            let res = libc::sem_trywait(raw(self));
            if res == 0 {
                break;
            } else if *libc::__errno_location() != libc::EINTR {
                panic!("semaphore internal error during wait()");
            }
        }
    }
    */

    /// Blocks until the semaphore can be decremented.
    pub fn wait(&self) {
        loop {
            unsafe {
                let res = libc::sem_wait(raw(self));
                if res == 0 {
                    break;
                } else if *libc::__errno_location() != libc::EINTR {
                    panic!("semaphore internal error during wait()");
                }
            }
        }
    }

    /// Increments the semaphore by one.
    pub fn post(&self) {
        let res = unsafe { libc::sem_post(raw(self)) };
        if res != 0 {
            panic!("semaphore internal error during post()");
        }
    }   
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        let r = unsafe { libc::sem_destroy(raw(self)) };
        debug_assert_eq!(r, 0);
    }
}

unsafe impl Send for Semaphore {}

unsafe impl Sync for Semaphore {}

/*
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


// Per-process state and interprocess state should be separate.
// Per-process `local` state should only be accessible via a
// custom boundary.


pub struct FizzleCell {
    local_locks: [Option<Sem>; 128],
    inner: UnsafeCell<FizzleState>,
}

pub struct FizzleState {
    pub local: FizzleLocal,
    pub global: FizzleGlobal,
}

impl FizzleState {
    fn acquire(&self) -> FizzleGuard<'_> {
        todo!()
    }
}

pub struct FizzleGuard<'a> {
    inner: &'a FizzleCell,
}

pub struct FizzleLocal {

}

impl FizzleLocal {

}

pub struct FizzleGlobal {

}


*/
