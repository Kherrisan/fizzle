use std::cmp::Ordering;
use std::hash::Hash;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::{array, cmp, mem, ptr};

#[derive(Debug, Clone, Eq)]
pub struct Buffer<const T: usize> {
    data: [u8; T],
    data_len: usize,
}

impl<const T: usize> PartialEq for Buffer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.data[..self.data_len] == other.data[..other.data_len]
    }
}

impl<const T: usize> Hash for Buffer<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.data[..self.data_len].hash(state);
        self.data_len.hash(state);
    }
}

impl<const T: usize> Default for Buffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const T: usize> Buffer<T> {
    pub fn new() -> Self {
        Self {
            data: [0u8; T],
            data_len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.data_len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == T
    }

    pub fn shrink(&mut self, new_length: usize) -> Result<(), BufferError> {
        if self.data_len < new_length {
            return Err(BufferError);
        }

        self.data_len = new_length;
        Ok(())
    }

    pub fn data(&self) -> &[u8] {
        &self.data[..self.data_len]
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data[..self.data_len]
    }

    /// Places `data` in the buffer, clearing out any prior data in the process.
    pub fn put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        self.data[..data.len()].copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    /// Attempts to place `data` in the buffer, clearing out any prior data in the process.
    pub fn try_put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(..data.len()) else {
            return Err(BufferError);
        };

        write_slice.copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    pub fn append(&mut self, data: &[u8]) {
        self.data[self.data_len..self.data_len + data.len()].copy_from_slice(data);
        self.data_len += data.len();
    }

    pub fn try_append(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(self.data_len..self.data_len + data.len()) else {
            return Err(BufferError);
        };

        write_slice.copy_from_slice(data);
        self.data_len += data.len();
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RingBuffer<const T: usize> {
    data: [MaybeUninit<u8>; T],
    data_idx: usize,
    data_len: usize,
}

impl<const T: usize> Hash for RingBuffer<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let end_idx = self.data_idx + self.data_len;
        let first_end = cmp::min(end_idx, T);
        (unsafe {
            &*(&self.data[self.data_idx..first_end] as *const [MaybeUninit<u8>] as *const [u8])
        })
        .hash(state);

        if end_idx > T {
            (unsafe { &*(&self.data[..end_idx % T] as *const [MaybeUninit<u8>] as *const [u8]) })
                .hash(state);
        }
    }
}

impl<const T: usize> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const T: usize> RingBuffer<T> {
    pub fn new() -> Self {
        Self {
            data: array::from_fn(|_| MaybeUninit::uninit()),
            data_idx: 0,
            data_len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.data_len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == T
    }

    pub fn clear(&mut self) {
        self.data_idx = 0;
        self.data_len = 0;
    }

    pub fn write(&mut self, buf: &[u8]) -> usize {
        if self.data_len == T {
            return 0
        }

        // TODO: this (and `read()`) can both use all the I/O they've been provided; fix this!
        let end_idx = (self.data_idx + self.data_len) % T;

        let available = match end_idx.cmp(&self.data_idx) {
            Ordering::Greater | Ordering::Equal => T - end_idx,
            Ordering::Less => self.data_idx - end_idx,
        };

        let written = cmp::min(available, buf.len());

        self.data[end_idx..end_idx + written].copy_from_slice(unsafe {
            &*(&buf[..written] as *const [u8] as *const [MaybeUninit<u8>])
        });
        self.data_len += written;
        written
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        if self.data_len == 0 {
            return 0
        }

        let available = cmp::min(self.data_len, T - self.data_idx);
        let read = cmp::min(available, buf.len());

        buf[..read].copy_from_slice(unsafe {
            &*(&self.data[self.data_idx..self.data_idx + read] as *const [MaybeUninit<u8>]
                as *const [u8])
        });
        self.data_idx = (self.data_idx + read) % T;

        read
    }
}

/// A set of values that can be indexed into by a key of type `K`.
///
#[derive(Debug)]
pub struct ValueIndex<K: Sized + From<usize> + Into<usize>, V: Sized, const N: usize> {
    inner: [Option<V>; N],
    next_key: usize,
    _phantom: PhantomData<K>,
}

impl<K: Sized + From<usize> + Into<usize>, V: Sized, const N: usize> ValueIndex<K, V, N> {
    pub fn new() -> Self {
        Self {
            inner: array::from_fn(|_| None),
            next_key: 0usize,
            _phantom: Default::default(),
        }
    }

    pub unsafe fn initialize(value_idx: *mut ValueIndex<K, V, N>) {
        for i in 0..N {
            *ptr::addr_of_mut!((*value_idx).inner[i]) = None;
        }
        *ptr::addr_of_mut!((*value_idx).next_key) = 0;
        *ptr::addr_of_mut!((*value_idx)._phantom) = Default::default();
    }

    pub fn get(&self, key: K) -> Option<&V> {
        let key: usize = key.into();
        if key >= self.inner.len() {
            return None;
        }
        self.inner[key].as_ref()
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.inner[key.into()].as_mut()
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let mut res = Some(value);
        mem::swap(&mut res, &mut self.inner[key.into()]);
        res
    }

    pub fn put(&mut self, value: V) -> K {
        let Some(key) = self.next_key() else {
            panic!("ValueIndex structure out of space");
        };

        self.inner[key] = Some(value);
        K::from(key)
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let mut res = None;
        mem::swap(&mut res, &mut self.inner[key.into()]);
        res
    }

    /// Retrieves the next available key from the value index.
    ///
    /// This algorithm has an average temporal complexity of O(N/K), where N is the number of
    /// places in the ValueIndex and K is the average number of available slots at the time a key
    /// needs to be procured over many trials. So, assuming you only use half of the available
    /// capacity of your Value Index, the average key procurement time should be N / (N / 2) = 2
    /// indexes.
    fn next_key(&mut self) -> Option<usize> {
        let mut curr_key = self.next_key;
        while self.inner[curr_key].is_some() {
            curr_key = (curr_key + 1) % N;
            if curr_key == self.next_key {
                // All keys are exhausted
                return None;
            }
        }
        self.next_key = (curr_key + 1) % N;
        Some(curr_key)
    }
}

impl<K: Sized + From<usize> + Into<usize>, V: Sized, const N: usize> Default
    for ValueIndex<K, V, N>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Sized + From<usize> + Into<usize> + Clone, V: Sized + Clone, const N: usize> Clone
    for ValueIndex<K, V, N>
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            next_key: self.next_key,
            _phantom: self._phantom,
        }
    }
}

#[derive(Debug)]
pub struct BufferError;
