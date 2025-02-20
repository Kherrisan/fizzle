use std::alloc::{Allocator, Global};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::rc::Rc;

unsafe fn raw(s: &Semaphore) -> *mut libc::sem_t {
    s.inner.get().cast()
}

#[derive(Debug)]
pub struct Semaphore {
    inner: UnsafeCell<libc::sem_t>,
}

impl Semaphore {
    /// Constructs a new semaphore within a boxed memory region.
    ///
    /// This method is safe to use under most normal circumstances, but the enclosed `Sem` must
    /// not be moved out of the box or undefined behavior will occur.
    #[inline]
    pub fn new_boxed(value: u32) -> Box<Semaphore> {
        Self::new_boxed_in(value, false, Global)
    }

    /// Constructs a new semaphore within a boxed memory region from the provided allocator.
    ///
    /// This method is safe to use under most normal circumstances, but the enclosed `Sem` must
    /// not be moved out of the box or undefined behavior will occur.
    pub fn new_boxed_in<A>(value: u32, shared: bool, alloc: A) -> Box<Semaphore, A>
    where
        A: Allocator,
    {
        let mut s: Box<MaybeUninit<Semaphore>, A> = Box::new_in(MaybeUninit::uninit(), alloc);

        Self::initialize(&mut s, shared, value);

        unsafe { s.assume_init() }
    }

    /// Constructs a new semaphore within an `Rc` memory region.
    ///
    /// This method is safe to use under most normal circumstances, but the enclosed `Sem` must
    /// not be moved out of the box or undefined behavior will occur.
    #[inline]
    pub fn new_rc(value: u32) -> Rc<Semaphore> {
        Self::new_rc_in(value, false, Global)
    }

    /// Constructs a new semaphore within an `Rc` memory region from the provided allocator.
    ///
    /// This method is safe to use under most normal circumstances, but the enclosed `Sem` must
    /// not be moved out of the box or undefined behavior will occur.
    pub fn new_rc_in<A>(value: u32, shared: bool, alloc: A) -> Rc<Semaphore, A>
    where
        A: Allocator,
    {
        let mut s: Rc<MaybeUninit<Semaphore>, A> = Rc::new_in(MaybeUninit::uninit(), alloc);

        // SAFETY: `s` is the only reference, so `Rc::get_mut()` is guaranteed to return `Some()`.
        Self::initialize(Rc::get_mut(&mut s).unwrap(), shared, value);

        unsafe { s.assume_init() }
    }

    /// Initializes a semaphore in-place.
    ///
    /// This method is safe to use with shared memory to enable inter-process communication locks.
    pub fn initialize(
        sem: &mut MaybeUninit<Semaphore>,
        shared: bool,
        value: u32,
    ) -> &mut Semaphore {
        let access = match shared {
            true => libc::PTHREAD_PROCESS_SHARED,
            false => libc::PTHREAD_PROCESS_PRIVATE,
        };

        // TODO: is this sound? Run Miri...
        unsafe {
            let sem_ptr = (&raw mut (*sem.as_mut_ptr()).inner).cast::<libc::sem_t>();
            // Initialize the `sem_t` contained within the UnsafeCell
            assert_eq!(libc::sem_init(sem_ptr, access, value), 0);
            // UnsafeCell<libc::sem_t> is now fully initialized
            sem.assume_init_mut()
        }
    }

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
        let ret = unsafe { libc::sem_destroy(raw(self)) };
        debug_assert_eq!(ret, 0);
    }
}

// unsafe impl Send for Semaphore {}

unsafe impl Sync for Semaphore {}
