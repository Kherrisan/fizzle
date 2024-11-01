use std::{cell::UnsafeCell, mem::MaybeUninit};

/// A `OnceCell` that guarantees safety if and only if all threads within the process run
/// sequentially rather than in parallel.
///
///
pub struct SeqOnceCell<T> {
    inner: UnsafeCell<Option<T>>,
}

impl<T> SeqOnceCell<T> {
    pub const fn new() -> SeqOnceCell<T> {
        SeqOnceCell {
            inner: UnsafeCell::new(None),
        }
    }

    pub fn get(&self) -> Option<&T> {
        unsafe { &*self.inner.get() }.as_ref()
    }

    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        match self.get() {
            Some(val) => val,
            None => {
                let slot = unsafe { &mut *self.inner.get() };
                slot.insert(f())
            }
        }
    }

    pub fn get_or_situate<F>(&self, f: F) -> &T
    where
        F: FnOnce(&mut MaybeUninit<T>) -> &mut T,
    {
        match self.get() {
            Some(val) => val,
            None => {
                let slot = unsafe { &mut *(self.inner.get() as *mut Option<MaybeUninit<T>>) };
                // Set value to Some() with uninitialized memory
                let uninit = slot.insert(MaybeUninit::uninit());
                // Initialize the memory
                f(uninit)
            }
        }
    }
}

unsafe impl<T> Sync for SeqOnceCell<T> {}
