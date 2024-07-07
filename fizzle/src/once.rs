use std::{cell::UnsafeCell, mem::MaybeUninit};

/// Non-concurrent OnceCell.
/// 
/// 
pub struct NcOnceCell<T> {
    inner: UnsafeCell<Option<T>>,
}

impl<T> NcOnceCell<T> {
    pub const fn new() -> NcOnceCell<T> {
        NcOnceCell { inner: UnsafeCell::new(None) }
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
        F: FnOnce(&mut MaybeUninit<T>) -> &mut T
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

unsafe impl<T> Sync for NcOnceCell<T> {}