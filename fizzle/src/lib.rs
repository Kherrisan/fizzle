//#![feature(c_variadic)]

extern crate libc;

mod hook_macros;
mod hooks;
mod semaphore;
mod state;
mod streams;

pub(crate) use hook_macros::hook;

use std::{ffi::CStr, hash::Hash, os::fd::RawFd, process, ptr};

#[derive(Debug)]
pub struct BufferError {
    pub reason: &'static str,
}

// Future work: make the state variable `Sized` so that it can be constructed in shared memory for multi-process fuzzing
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
pub struct FilePathError {
    pub reason: &'static str,
}

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

    pub fn from_cstr(path: &CStr) -> Result<Self, FilePathError> {
        Self::from_raw_bytes(path.to_bytes())
    }

    /// Note that this should not include any null terminating character.
    pub fn from_raw_bytes(path: &[u8]) -> Result<Self, FilePathError> {
        if path.len() > 255 {
            return Err(FilePathError {
                reason: "filepath exceeded 255 character max size",
            });
        }

        let mut buf = Buffer::new();
        buf.try_put(path)
            .map_err(|e| FilePathError { reason: e.reason })?;

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
                        b"/" => {
                            return Err(FilePathError {
                                reason: "backtrack attempted on root path",
                            })
                        }
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
            return Err(FilePathError {
                reason: "empty path",
            });
        }

        let trailing_slash = data[write_idx - 1] == b'/';

        data[write_idx] = b'\0';
        write_idx += 1;

        buf.shrink(write_idx)
            .map_err(|e| FilePathError { reason: e.reason })?;

        Ok(FilePath {
            buf,
            trailing_slash,
        })
    }

    pub fn concat(mut self, other: &FilePath) -> Result<Self, FilePathError> {
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
                        b"" | b"../" => self.buf.try_append(b"../").map_err(|_| FilePathError {
                            reason: "insufficient space",
                        })?,
                        b"/" => {
                            return Err(FilePathError {
                                reason: "backtrack attempted on root path",
                            })
                        }
                        segment => self.buf.shrink(segment.len()).unwrap(),
                    }
                }
                _ => {
                    self.buf.try_append(segment).map_err(|_| FilePathError {
                        reason: "insufficient space",
                    })?;
                    // copy '/' if exists
                    if segment_len < data.len() - read_idx {
                        self.buf.try_append(b"/").map_err(|_| FilePathError {
                            reason: "insufficient space",
                        })?;
                    }
                }
            }

            read_idx += segment_len + 1;
        }

        // Re-add null character
        self.buf.try_append(b"\0").map_err(|_| FilePathError {
            reason: "insufficient space",
        })?;

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

/// Abort the process immediately, printing `reason` to stderr.
pub(crate) fn abort(reason: &'static str) -> ! {
    eprintln!("Fatal: {}", reason);
    process::exit(-1);
}

/// Abort the process if the `FIZZLE_ABORT` environment variable is equal to 1.
pub(crate) fn debug_abort(function_name: &'static str) {
    if false {
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
