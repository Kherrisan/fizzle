//#![feature(c_variadic)]

extern crate libc;

mod hook_macros;
mod hooks;
mod semaphore;
mod state;
mod streams;

pub(crate) use hook_macros::hook;

use std::cmp::Ordering;
use std::ffi::CStr;
use std::hash::Hash;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::{array, cmp, io, mem, process, ptr};

/// A set of values that can be indexed into by a key of type `K`.
///
#[derive(Debug)]
pub struct ValueIndex<K: Sized + From<usize> + Into<usize>, V: Sized, const N: usize> {
    inner: [Option<V>; N],
    next_key: usize,
    _phantom: PhantomData<K>,
}

impl<K: Sized + From<usize> + Into<usize>, V: Sized, const N: usize> ValueIndex<K, V, N> {
    fn next_key(&mut self) -> Option<usize> {
        let mut curr_key = self.next_key;
        while self.inner[curr_key].is_some() {
            curr_key = (curr_key + 1) % N;
            if curr_key == self.next_key {
                return None;
            }
        }
        self.next_key = (curr_key + 1) % N;
        Some(curr_key)
    }

    pub fn new() -> Self {
        Self {
            inner: array::from_fn(|_| None),
            next_key: 0usize,
            _phantom: Default::default(),
        }
    }

    pub fn get(&self, key: K) -> Option<&V> {
        self.inner[key.into()].as_ref()
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
pub struct BufferError {
    pub reason: &'static str,
}

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

    pub fn shrink(&mut self, new_length: usize) -> Result<(), BufferError> {
        if self.data_len < new_length {
            return Err(BufferError {
                reason: "shrink() called with length greater than buffer",
            });
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

    pub fn put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        self.data[..data.len()].copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    pub fn try_put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(..data.len()) else {
            return Err(BufferError {
                reason: "insufficient size",
            });
        };

        write_slice.copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    pub fn append(&mut self, data: &[u8]) {
        self.data[..data.len()].copy_from_slice(data);
        self.data_len = data.len();
    }

    pub fn try_append(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(self.data_len..self.data_len + data.len()) else {
            return Err(BufferError {
                reason: "insufficient size",
            });
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

impl<const T: usize> Write for RingBuffer<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.data_len == T {
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }

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
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<const T: usize> Read for RingBuffer<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.data_len == 0 {
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }

        let available = cmp::min(self.data_len, T - self.data_idx);
        let read = cmp::min(available, buf.len());

        buf[..read].copy_from_slice(unsafe {
            &*(&self.data[self.data_idx..self.data_idx + read] as *const [MaybeUninit<u8>]
                as *const [u8])
        });
        self.data_idx = (self.data_idx + read) % T;

        Ok(read)
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

    pub fn clear(&mut self) {
        self.data_idx = 0;
        self.data_len = 0;
    }
}

#[derive(Debug, Clone)]
pub struct PathError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FilePath {
    buf: Buffer<256>,
    trailing_slash: bool,
}

impl Default for FilePath {
    fn default() -> Self {
        let mut buf = Buffer::new();
        buf.append(b"/");

        Self {
            buf,
            trailing_slash: true,
        }
    }
}

impl FilePath {
    fn segment(path: &[u8]) -> &[u8] {
        for (idx, &c) in path.iter().enumerate() {
            if c == b'/' {
                return &path[..idx];
            }
        }
        path
    }

    // Gets the reverse segment of the given path
    fn last_segment(path: &[u8]) -> &[u8] {
        let mut first_slash_seen = false;
        for (idx, &c) in path.iter().enumerate().rev() {
            if c == b'/' {
                if first_slash_seen {
                    return &path[idx + 1..];
                } else {
                    first_slash_seen = true;
                }
            }
        }
        path
    }

    pub fn from_cstr(path: &CStr) -> Result<Self, PathError> {
        Self::from_raw_bytes(path.to_bytes())
    }

    /// Note that this should not include any null terminating character.
    pub fn from_raw_bytes(path: &[u8]) -> Result<Self, PathError> {
        if path.len() > 255 {
            return Err(PathError);
        }

        let mut buf = Buffer::new();
        buf.try_put(path).map_err(|_| PathError)?;

        let mut read_idx = 0usize;
        let mut write_idx = 0usize;
        let data = buf.data_mut();

        // Special case: path is absolute
        if let Some(b'/') = data.get(read_idx) {
            read_idx += 1;
            write_idx += 1;
        }

        while read_idx < data.len() {
            let segment = Self::segment(&data[read_idx..]);
            let segment_len = segment.len();
            match segment {
                b"" | b"." => (), // Do nothing
                b".." => {
                    // Traverse back one segment
                    match Self::last_segment(&data[..write_idx]) {
                        b"" | b"../" => {
                            data.copy_from_slice(b"../");
                            write_idx += 3;
                        }
                        b"/" => return Err(PathError),
                        segment => write_idx -= segment.len(),
                    }
                }
                _ => {
                    // Copy current segment to write portion
                    for i in 0..segment_len {
                        data[write_idx + i] = data[read_idx + i];
                    }
                    write_idx += segment_len;

                    // copy '/' if exists
                    if segment_len < data.len() - read_idx {
                        data[write_idx] = b'/';
                        write_idx += 1;
                    }
                }
            }

            read_idx += segment_len + 1;
        }

        if write_idx == 0 {
            return Err(PathError);
        }

        let trailing_slash = data[write_idx - 1] == b'/';

        data[write_idx] = b'\0';
        write_idx += 1;

        buf.shrink(write_idx).map_err(|_| PathError)?;

        Ok(FilePath {
            buf,
            trailing_slash,
        })
    }

    pub fn concat(mut self, other: &FilePath) -> Result<Self, PathError> {
        let data = &other.buf.data()[..other.buf.data().len() - 1]; // remove null character
        let mut read_idx = 0;

        self.buf.shrink(self.buf.len() - 1).unwrap(); // Remove null character

        while read_idx < other.buf.len() {
            let segment = Self::segment(&data[read_idx..]);
            let segment_len = segment.len();

            match segment {
                b"" | b"." => (), // Do nothing (shouldn't happen unless `other` has an absolute at the start)
                b".." => {
                    // Traverse back one segment
                    match Self::last_segment(self.buf.data()) {
                        b"" | b"../" => self.buf.try_append(b"../").map_err(|_| PathError)?,
                        b"/" => return Err(PathError),
                        segment => self.buf.shrink(segment.len()).unwrap(),
                    }
                }
                _ => {
                    self.buf.try_append(segment).map_err(|_| PathError)?;
                    // copy '/' if exists
                    if segment_len < data.len() - read_idx {
                        self.buf.try_append(b"/").map_err(|_| PathError)?;
                    }
                }
            }

            read_idx += segment_len + 1;
        }

        // Re-add null character
        self.buf.try_append(b"\0").map_err(|_| PathError)?;

        self.trailing_slash = other.trailing_slash;
        Ok(self)
    }

    pub fn is_absolute(&self) -> bool {
        self.buf.data().first() == Some(&b'/')
    }

    pub fn has_trailing_slash(&self) -> bool {
        self.trailing_slash
    }
}

/// The path for a named semaphore.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemPath {
    buf: Buffer<252>,
}

impl SemPath {
    pub fn from_cstr(path: &CStr) -> Result<Self, PathError> {
        Self::from_raw_bytes(path.to_bytes_with_nul())
    }

    /// Note that this **should** include a null terminating character.
    pub fn from_raw_bytes(path: &[u8]) -> Result<Self, PathError> {
        if path.len() > 252 {
            return Err(PathError);
        }

        let Some(b'/') = path.first() else {
            return Err(PathError);
        };

        let Some(b'\0') = path.last() else {
            return Err(PathError);
        };

        for &b in path.iter().skip(1).take(path.len() - 2) {
            if b == b'/' || b == b'\0' {
                return Err(PathError);
            }
        }

        let mut buf = Buffer::new();
        buf.append(path);

        Ok(Self { buf })
    }

    pub fn as_cstr(&self) -> &CStr {
        unsafe { CStr::from_bytes_with_nul_unchecked(&self.buf.data) }
    }
}

/// Abort the process immediately, printing `reason` to stderr.
pub(crate) fn abort(reason: &'static str) -> ! {
    eprintln!("Fatal: {}", reason);
    process::exit(-1);
}

/// Abort the process if the `FIZZLE_ABORT` environment variable is equal to 1.
pub(crate) fn debug_abort(function_name: &'static str) {
    if state::fizzle_debug_enabled() {
        eprintln!("Fatal: unimplemented shim `{}`", function_name);
        process::exit(-1);
    }
}

#[macro_export]
macro_rules! trace_enter {
    ($f:tt) => {
        if state::fizzle_trace_enabled() {
            eprintln!(
                "Thread {:?} invoked function {}",
                std::thread::current().id(),
                stringify!($f)
            );
        }
    };
}

#[macro_export]
macro_rules! trace_exit {
    ($f:tt) => {
        if state::fizzle_trace_enabled() {
            eprintln!(
                "Thread {:?} leaving function {}",
                std::thread::current().id(),
                stringify!($f)
            );
        }
    };
}

/// Creates a new location in memory that is guaranteed to be unique to others.
/// This is particularly useful in emulating hooks that require a pointer as a return value.
/// Memory locations should be destroyed with `unique_mem_destroy()` once finished using.
unsafe fn unique_mem_create() -> *mut libc::c_void {
    // TODO: turn this into an alias creator that uses sequential addresses in allocated to handle these opaque references more efficiently.

    let addr = libc::mmap(
        ptr::null_mut(),
        1,
        libc::PROT_NONE,
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
        -1,
        0,
    );
    if addr.is_null() {
        abort("failed to create unique memory handle via `mmap`");
    }

    addr
}

/// Unmaps a location in memory created with `unique_mem_create()`.
/// This uses `munmap` under the hood; it is unsafe to call this on any `mem_location` other than those returned by `unique_mem_create()`.
unsafe fn unique_mem_destroy(mem_location: *mut libc::c_void) {
    let res = unsafe { libc::munmap(mem_location, 1) };
    if res != 0 {
        abort("error during destruction of unique memory handle via `mmap`");
    }
}

fn alias_fd_create() -> RawFd {
    let fd = unsafe { libc::memfd_create(c"FIZZLE_ALIAS_FD".as_ptr(), 0) };
    if fd < 0 {
        abort("fizzle internal file descriptor alias creation (`memfd_create`) failed");
    }
    fd
}

fn alias_fd_destroy(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}
