use std::cell::{Ref, RefCell, RefMut, UnsafeCell};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, Ordering};

/// A mutable memory location meant to be only used in single-threaded contexts.
///
/// # Safety
///
/// Although `SequentialRefCell` is marked as `Sync`, only one thread may access
/// it at a time. Failure to enforce this rule will result in Undefined Behavior.
pub struct SequentialRefCell<T> {
    inner: RefCell<T>,
}

impl<T> SequentialRefCell<T> {
    /// Creates a new `SequentialRefCell` containing a value.
    pub fn new(inner: T) -> Self {
        Self {
            inner: RefCell::new(inner),
        }
    }

    /// Immutably borrows a wrapped value.
    #[inline]
    pub fn borrow(&self) -> Ref<'_, T> {
        self.inner.borrow()
    }

    /// Mutably borrows a wrapped value.
    #[inline]
    pub fn borrow_mut(&self) -> RefMut<'_, T> {
        self.inner.borrow_mut()
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

unsafe impl<T> Sync for SequentialRefCell<T> {}

pub struct PanicOnceCell<T> {
    inner: UnsafeCell<MaybeUninit<T>>,
    state: AtomicU8,
}

impl<T> PanicOnceCell<T> {
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(0b0000_0000),
        }
    }

    pub fn get(&self) -> Option<&T> {
        let state = self.state.load(Ordering::Acquire);
        if state == 0b0000_0000 {
            // 1st MSB not currently set: need to initialize
            return None;
        } else if state == 0b0000_0001 {
            // 1st MSB set: initialization underway but not complete (panic)
            panic!("PanicOnceCell accessed while being initialized in another context")
        } else {
            // 2nd MSB set: initialization complete
            unsafe { Some(&*(self.inner.get().cast_const().cast::<T>())) }
        }
    }

    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        // These are `Ordering::Relaxed` because this is assumed to only ever be called
        // in a single-threaded context. Synchronization of data already happens with Fizzle
        // semaphores.
        let state = self.state.fetch_xor(0b0000_0001, Ordering::Relaxed);
        if state == 0b0000_0000 {
            // 1st MSB not currently set: need to initialize
            self.initialize(f);

            unsafe { &*(self.inner.get().cast_const().cast::<T>()) }
        } else if state == 0b0000_0001 {
            // 1st MSB set: initialization underway but not complete (panic)
            panic!("OnceCell accessed while being initialized in another context")
        } else {
            // 2nd MSB set: initialization complete
            unsafe { &*(self.inner.get().cast_const().cast::<T>()) }
        }
    }

    pub unsafe fn deinit(&self) -> T {
        if self.state.fetch_and(0b0000_0000, Ordering::Relaxed) & 0b0000_0010 == 0 {
            panic!("OnceCell deinit() called when not fully initialized")
        }

        (*self.inner.get()).assume_init_read()
    }

    // `inline(never)` and `cold` improve branch prediction, since `initialize()` only ever happens
    // once on program startup.
    #[inline(never)]
    #[cold]
    fn initialize<F>(&self, f: F)
    where
        F: FnOnce() -> T,
    {
        unsafe {
            (&mut *self.inner.get()).write(f());
        }

        // Set 2nd MSB to indicate initialization is complete
        self.state.fetch_xor(0b0000_0010, Ordering::Relaxed);
    }
}

unsafe impl<T: Sync> Sync for PanicOnceCell<T> {}
