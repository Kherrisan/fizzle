use std::hash::{Hash, Hasher};
use std::thread::ThreadId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadTermination {
    Cancellation,
    Exit(*mut libc::c_void),
    #[allow(unused)]
    SigTerm, // TODO: implement
}

pub type PThreadDestructor = unsafe extern "C" fn(*mut libc::c_void);

#[derive(Clone, Copy, Debug)]
pub struct PThreadRoutine {
    pub function: PThreadDestructor,
    pub arg: Option<*mut libc::c_void>,
}

impl PThreadRoutine {
    /// Calls the given routine
    pub fn call(self) {
        if let Some(arg) = self.arg {
            unsafe {
                (self.function)(arg);
            }
        }
    }
}

/// A hasher that correctly outputs the internal value of a [`ThreadId`] for its hash.
pub struct ThreadHasher {
    value: u64,
}

impl ThreadHasher {
    pub fn new() -> Self {
        Self { value: 0 }
    }
}

impl Hasher for ThreadHasher {
    fn finish(&self) -> u64 {
        self.value
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut idx = 0usize;
        while bytes.len() - idx >= 8 {
            let bytearray: [u8; 8] = bytes[idx..idx + 8].try_into().unwrap();
            self.value += u64::from_le_bytes(bytearray);
            idx += 8;
        }

        if idx != bytes.len() {
            let mut bytearray = [0u8; 8];
            for (i, b) in bytes[idx..].iter().rev().enumerate() {
                bytearray[i] = *b;
            }
            self.value += u64::from_le_bytes(bytearray);
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.value += i as u64;
    }

    fn write_u64(&mut self, i: u64) {
        self.value += i;
    }
}

pub fn index_of_thread(thread: &ThreadId) -> usize {
    let mut hasher = ThreadHasher::new();
    thread.hash(&mut hasher);
    hasher.finish() as usize
}
