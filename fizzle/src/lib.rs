//#![feature(c_variadic)]

extern crate libc;

mod hook_macros;
mod hooks;
mod scheduler;
mod state;
mod streams;

pub(crate) use hook_macros::hook;

use std::{ffi::CStr, process, ptr};


pub struct BufferError {
    pub reason: &'static str,
}

// Future work: make the state variable `Sized` so that it can be constructed in shared memory for multi-process fuzzing
#[derive(Debug, Clone)]
pub struct Buffer<const T: usize> {
    data: [u8; T],
    data_len: usize,
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

    pub fn shrink(&mut self, new_length: usize) -> Result<(), BufferError> {
        if self.data_len < new_length {
            return Err(BufferError { reason: "shrink() called with length greater than buffer" })
        }

        self.data_len = new_length;
        Ok(())
    }

    pub fn data(&self) -> &[u8] {
        &self.data[..self.data_len]
    }

    pub fn data_mut(&self) -> &mut [u8] {
        &mut self.data[..self.data_len]
    }

    pub fn put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        write_slice[..data.len()].copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    pub fn try_put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(..data.len()) else {
            return Err(BufferError { reason: "insufficient size" })
        };

        write_slice.copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    pub fn append(&mut self, data: &[u8]) {
        self.data[..data.len()].copy_from_slice(data);
        self.data_len = data.len();
        Ok(())
    }

    pub fn try_append(&mut self, data: &[u8]) -> Result<(), BufferError> {
        let Some(write_slice) = self.data.get_mut(self.data_len..self.data_len + data.len()) else {
            return Err(BufferError { reason: "insufficient size" })
        };

        write_slice.copy_from_slice(data);
        self.data_len += data.len();
        Ok(())
    }

    pub fn append(&mut self, data: &[u8]) {
        self.data[self.data_len..self.data_len + data.len()].copy_from_slice(data);
        self.data_len += data.len();
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FilePathError {
    reason: &'static str,
}

#[derive(Debug, Clone)]
pub struct FilePath {
    buf: Buffer<256>,
}

impl FilePath {
    pub fn from_cstr(path: &CStr) -> Result<Self, FilePathError> {


        let mut buf = Buffer::new();
        buf.try_put(path.to_bytes_with_nul()).map_err(|e| FilePathError { reason: e.reason })?;

        let mut read_idx = 0usize;
        let mut write_idx = 0usize;
        let data = buf.data_mut();

        while let Some(&c) = data.get(write_idx) {
            
        }



        Ok(FilePath { buf })
    }

    pub fn concat(&mut self, other: &FilePath) -> Result<(), FilePathError> {
        todo!()
    }
}



/// Abort the process immediately, printing `reason` to stderr.
pub(crate) fn abort(reason: &'static str) -> ! {
    eprintln!("Fatal: {}", reason);
    process::exit(-1);
}

/// Abort the process if the `FIZZLE_ABORT` environment variable is equal to 1.
pub(crate) fn debug_abort(function_name: &'static str) {
    if state::fizzle_state().lock().unwrap().debug_enabled {
        eprintln!("Fatal: unimplemented shim `{}`", function_name);
        process::exit(-1);
    }
}

#[macro_export]
macro_rules! trace_enter {
    ($f:tt) => { if state::fizzle_trace_enabled() { eprintln!("Thread {:?} entering function {}", std::thread::current().id(), stringify!($f)); } }
}

#[macro_export]
macro_rules! trace_exit {
    ($f:tt) => { if state::fizzle_trace_enabled() { eprintln!("Thread {:?} leaving function {}", std::thread::current().id(), stringify!($f)); } }
}

/// Creates a new location in memory that is guaranteed to be unique to others.
/// This is particularly useful in emulating hooks that require a pointer as a return value.
/// Memory locations should be destroyed with `unique_mem_destroy()` once finished using.
unsafe fn unique_mem_create() -> *mut libc::c_void {
    let addr = libc::mmap(ptr::null_mut(), 1, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
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
