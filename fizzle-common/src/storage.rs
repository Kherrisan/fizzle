use std::fmt::Debug;
use std::hash::Hash;
use std::mem::MaybeUninit;
use std::{cmp, slice};

unsafe fn slice_init(slice: &[MaybeUninit<u8>]) -> &[u8] {
    slice::from_raw_parts(slice.as_ptr() as *const u8, slice.len())
}

unsafe fn slice_init_mut(slice: &mut [MaybeUninit<u8>]) -> &mut [u8] {
    slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut u8, slice.len())
}

pub struct Buffer<const T: usize> {
    data: [MaybeUninit<u8>; T],
    data_start: usize,
    data_end: usize,
}

impl<const N: usize> Debug for Buffer<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unsafe { slice_init(&self.data[self.data_start..self.data_end]).fmt(f) }
    }
}

impl<const T: usize> Clone for Buffer<T> {
    fn clone(&self) -> Self {
        let mut data: [MaybeUninit<u8>; T] = unsafe { MaybeUninit::uninit().assume_init() };
        data[self.data_start..self.data_end]
            .copy_from_slice(&self.data[self.data_start..self.data_end]);

        Self {
            data,
            data_start: self.data_start,
            data_end: self.data_end,
        }
    }
}

impl<const N: usize> PartialEq for Buffer<N> {
    fn eq(&self, other: &Self) -> bool {
        unsafe {
            slice_init(&self.data[self.data_start..self.data_end])
                == slice_init(&other.data[other.data_start..other.data_end])
        }
    }
}

impl<const N: usize> Eq for Buffer<N> {}

impl<const N: usize> Hash for Buffer<N> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        unsafe {
            slice_init(&self.data[self.data_start..self.data_end]).hash(state);
        }
    }
}

impl<const N: usize> Default for Buffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Buffer<N> {
    pub fn new() -> Self {
        Self {
            data: unsafe { MaybeUninit::uninit().assume_init() },
            data_start: 0,
            data_end: 0,
        }
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        assert!(slice.len() <= N);
        let mut buf = Self::new();
        buf.write(slice);
        buf
    }

    pub fn clear(&mut self) {
        self.data_start = 0;
        self.data_end = 0;
    }

    pub fn len(&self) -> usize {
        self.data_end - self.data_start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == N
    }

    pub fn shrink(&mut self, new_length: usize) -> Result<(), BufferError> {
        if (self.data_end - self.data_start) < new_length {
            return Err(BufferError);
        }

        self.data_end -= (self.data_end - self.data_start) - new_length;
        Ok(())
    }

    pub fn write_available(&self) -> usize {
        N - self.data_end
    }

    pub fn remaining_len(&self) -> usize {
        self.data.len() - self.data_end
    }

    pub fn remaining_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        &mut self.data[self.data_end..]
    }

    pub fn data(&self) -> &[u8] {
        unsafe { slice_init(&self.data[self.data_start..self.data_end]) }
    }

    pub fn did_read(&mut self, amount: usize) {
        match amount.cmp(&(self.data_end - self.data_start)) {
            cmp::Ordering::Less => self.data_start += amount,
            cmp::Ordering::Equal => {
                self.data_start = 0;
                self.data_end = 0;
            }
            cmp::Ordering::Greater => panic!("`did_read()` called with too large an amount"),
        }
    }

    pub fn write(&mut self, buf: &[u8]) -> usize {
        let amount = cmp::min(N - self.data_end, buf.len());

        for (dst, src) in self.data[self.data_end..].iter_mut().zip(buf) {
            dst.write(*src);
        }

        self.data_end += amount;
        amount
    }

    pub fn did_write(&mut self, amount: usize) {
        assert!(amount <= N - self.data_end);
        self.data_end += amount;
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let amount = cmp::min(self.data_end - self.data_start, buf.len());

        for (dst, src) in buf
            .iter_mut()
            .zip(&self.data[self.data_start..self.data_end])
        {
            *dst = unsafe { src.assume_init() };
        }

        if amount == self.data_end - self.data_start {
            self.data_start = 0;
            self.data_end = 0;
        } else {
            self.data_start += amount;
        }
        amount
    }

    pub fn read_uninit(&mut self, buf: &mut [MaybeUninit<u8>]) -> usize {
        let amount = cmp::min(self.data_end - self.data_start, buf.len());

        for (dst, src) in buf
            .iter_mut()
            .zip(&self.data[self.data_start..self.data_end])
        {
            dst.write(unsafe { src.assume_init() });
        }

        if amount == self.data_end - self.data_start {
            self.data_start = 0;
            self.data_end = 0;
        } else {
            self.data_start += amount;
        }
        amount
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        unsafe { slice_init_mut(&mut self.data[self.data_start..self.data_end]) }
    }

    /// Places `data` in the buffer, clearing out any prior data in the process.
    pub fn replace(&mut self, data: &[u8]) -> Result<(), BufferError> {
        unsafe {
            slice_init_mut(&mut self.data[..data.len()]).copy_from_slice(data);
        }
        self.data_start = 0;
        self.data_end = data.len();
        Ok(())
    }

    /// Attempts to place `data` in the buffer, clearing out any prior data in the process.
    pub fn try_replace(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(..data.len()) else {
            return Err(BufferError);
        };

        unsafe {
            slice_init_mut(write_slice).copy_from_slice(data);
        }
        self.data_start = 0;
        self.data_end = data.len();
        Ok(())
    }

    pub fn append(&mut self, data: &[u8]) {
        unsafe {
            slice_init_mut(&mut self.data[self.data_end..self.data_end + data.len()])
                .copy_from_slice(data);
        }
        self.data_end += data.len();
    }

    pub fn try_append(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(self.data_end..self.data_end + data.len()) else {
            return Err(BufferError);
        };

        unsafe {
            slice_init_mut(write_slice).copy_from_slice(data);
        }

        self.data_end += data.len();
        Ok(())
    }
}

#[derive(Debug)]
pub struct BufferError;
