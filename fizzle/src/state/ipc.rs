use std::ffi::{CStr, CString};
use std::os::fd::RawFd;
use std::{array, mem, ptr};

use crate::state::ProcessId;

const FIZZLE_MAX_PROCESSES: usize = 128;

/// A thread-safe and multiprocess-safe shared memory segment.
pub struct IpcMemory<T: Sized> {
    /// Per-process wait locks.
    proc_locks: [*mut libc::sem_t; FIZZLE_MAX_PROCESSES],
    data: *mut T,
    name: CString,
    memfd: RawFd,
}

impl<T: Sized> IpcMemory<T> {
    const SHMEM_LENGTH: usize =
        mem::size_of::<libc::pthread_mutex_t>() * FIZZLE_MAX_PROCESSES + mem::size_of::<T>();

    #[inline]
    pub fn new(inner: T) -> Self {
        let mut name = [0u8; 64];

        let rand_amount =
            unsafe { libc::getrandom(name[15..].as_mut_ptr() as *mut libc::c_void, 48, 0) };
        if rand_amount < 48 {
            crate::abort("insufficient entropy when initializing IpcMemory");
        }

        name[..15].copy_from_slice(b"/fizzle_shared_");
        for c in name.iter_mut().skip(15) {
            // Encode random characters to be [0-9?@A-Za-Z] (64 options)
            *c /= 4; // reduce options to 0..=63
            *c += 48;

            if *c >= 58 {
                *c += 5;
            }

            if *c >= 91 {
                *c += 6;
            }
        }

        let fd = unsafe {
            libc::shm_open(
                name.as_ptr() as *const i8,
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                libc::S_IRUSR | libc::S_IWUSR,
            )
        };
        if fd < 0 {
            crate::abort("unable to allocate shared memory for IpcMemory");
        }

        let name = unsafe { CStr::from_ptr(name.as_ptr() as *const i8).to_owned() };

        let mem_start = unsafe {
            libc::mmap(
                ptr::null_mut(),
                Self::SHMEM_LENGTH,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert!(
            mem_start != libc::MAP_FAILED,
            "unable to `mmap` shared memory for IpcMemory"
        );

        let mut proc_locks = [ptr::null_mut(); FIZZLE_MAX_PROCESSES];
        let mut sem_ptr = mem_start as *mut libc::sem_t;
        for proc_lock in proc_locks.iter_mut() {
            if unsafe { libc::sem_init(sem_ptr, libc::PTHREAD_PROCESS_SHARED, 1) } != 0 {
                crate::abort("unable to initialize per-process semaphores for IpcMemory");
            }
            *proc_lock = sem_ptr;
            unsafe { sem_ptr = sem_ptr.add(1) };
        }

        let data_ptr = sem_ptr as *mut T;

        unsafe {
            *data_ptr = inner;
        }

        Self {
            proc_locks,
            data: data_ptr,
            name,
            memfd: fd,
        }
    }

    pub fn from_identifier(name: &CStr) -> Self {
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDWR, 0) };
        assert!(
            fd >= 0,
            "unable to allocate shared memory for IpcMemory (`shm_open` failed)"
        );

        let name = name.to_owned();

        let mem_start = unsafe {
            libc::mmap(
                ptr::null_mut(),
                Self::SHMEM_LENGTH,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert!(
            mem_start != libc::MAP_FAILED,
            "unable to `mmap` shared memory for IpcMemory"
        );

        // Initialize per-process wait locks
        let mut sem_ptr = mem_start as *mut libc::sem_t;
        let proc_locks = array::from_fn(|_| {
            let ptr = sem_ptr;
            sem_ptr = unsafe { sem_ptr.add(1) };
            ptr
        });

        let data_ptr = sem_ptr as *mut T;

        Self {
            proc_locks,
            data: data_ptr,
            name,
            memfd: fd,
        }
    }

    /// Retrieves the identifier of the named shared memory.
    #[allow(unused)]
    pub fn shmem_name(&self) -> &CStr {
        &self.name
    }

    /// Retrieves a mutable reference to the data held within this IPC memory.
    pub fn data(&mut self) -> &mut T {
        unsafe { &mut *self.data }
    }

    /// Wakes up the process designated by `process_id`.
    pub fn process_wake(&self, process_id: ProcessId) {
        let proc_id: usize = process_id.into();
        assert!(
            proc_id < FIZZLE_MAX_PROCESSES,
            "internal fizzle process_wake function called with invalid ProcessId"
        );

        unsafe { libc::sem_post(self.proc_locks[proc_id]) };
    }

    /// Waits for the lock associated with `process_id` to be unlocked.
    pub fn process_wait(&self, process_id: ProcessId) {
        let proc_id: usize = process_id.into();
        assert!(
            proc_id < FIZZLE_MAX_PROCESSES,
            "internal fizzle process_wait function called with invalid ProcessId"
        );

        while unsafe { libc::sem_wait(self.proc_locks[proc_id]) } != 0 {}
    }

    #[allow(unused)]
    pub fn destroy(self) {
        let ret =
            unsafe { libc::munmap(self.proc_locks[0] as *mut libc::c_void, Self::SHMEM_LENGTH) };
        debug_assert!(ret == 0, "`munmap` failed while destroying IpcMemory");
        let ret = unsafe { libc::close(self.memfd) };
        debug_assert!(ret == 0, "`close` failed while destroying IpcMemory");
    }
}
