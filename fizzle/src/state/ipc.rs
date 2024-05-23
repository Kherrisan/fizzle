use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::{mem, ptr};

use fizzle_common::io::IoLocation;
use fizzle_plugin::IoLocationId;
use heapless::FnvIndexMap;

use super::InterprocessState;
use crate::constants::*;
use crate::state::ProcessId;

/// A thread-safe and multiprocess-safe shared memory segment.
pub struct IpcMemory {
    mem_start: *mut libc::c_void,
    /// Per-process wait locks.
    //proc_locks: [*mut libc::sem_t; FIZZLE_MAX_PROCESSES],
    //data: *mut T,
    name: CString,
    memfd: RawFd,
}

impl IpcMemory {
    const SHMEM_LENGTH: usize = (mem::size_of::<libc::sem_t>() * FIZZLE_MAX_PROCESSES)
        + mem::size_of::<InterprocessState>();

    #[inline]
    pub fn new(io_mapping: FnvIndexMap<IoLocation, IoLocationId, FIZZLE_MAX_PLUGINS>) -> Self {
        log::trace!("Initializing new IpcMemory");

        let mut name = [0u8; 64 + 1]; //

        let rand_amount = unsafe { libc::getrandom(name.as_mut_ptr() as *mut libc::c_void, 64, 0) };
        if rand_amount < 64 {
            panic!("`getrandom` failed during IpcMemory initialization");
        }

        name[..15].copy_from_slice(b"/fizzle_shared_");
        for c in name.iter_mut().skip(15).take(64 - 15) {
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
            panic!("unable to allocate shared memory for IpcMemory");
        }

        let res = unsafe { libc::ftruncate(fd, Self::SHMEM_LENGTH as libc::off_t) };
        if res != 0 {
            panic!("unexpected `ftruncate` error while allocating shared IpcMemory--insufficient memory?");
        }

        let name = unsafe { CStr::from_ptr(name.as_ptr() as *const i8).to_owned() };
        log::info!("Shared memory address: {:?}", name);

        let mem_start = unsafe {
            libc::mmap(
                ptr::null_mut(),
                Self::SHMEM_LENGTH as libc::size_t,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        log::debug!("shared memory allocated at offset {:?}", mem_start);

        if mem_start == libc::MAP_FAILED {
            panic!("unable to `mmap` shared memory for IpcMemory")
        }

        let ret = unsafe { libc::close(fd) };
        if ret != 0 {
            panic!("unexpected error from close");
        }

        let ret = unsafe { libc::shm_unlink(name.as_ptr()) };
        if ret != 0 {
            panic!("unexpected error from shm_unlink");
        }

        let mut sem_ptr = mem_start as *mut libc::sem_t;
        for _ in 0..FIZZLE_MAX_PROCESSES {
            unsafe {
                if libc::sem_init(sem_ptr, libc::PTHREAD_PROCESS_SHARED, 1) != 0 {
                    panic!("unable to initialize per-process semaphores for IpcMemory");
                }

                sem_ptr = sem_ptr.add(1);
            }
        }

        // This is a workaround because Rust doesn't have any equivalent of `placement new`.
        // TODO: maybe turn this into a full-blown proc macro crate?
        unsafe {
            InterprocessState::initialize(
                sem_ptr as *mut MaybeUninit<InterprocessState>,
                io_mapping,
            );
        }

        Self {
            mem_start,
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

        Self {
            mem_start,
            name,
            memfd: fd,
        }
    }

    /// Retrieves the identifier of the named shared memory.
    #[allow(unused)]
    pub fn shmem_name(&self) -> &CStr {
        &self.name
    }

    fn process_locks(&mut self) -> &mut [libc::sem_t; FIZZLE_MAX_PROCESSES] {
        unsafe { &mut *(self.mem_start as *mut [libc::sem_t; FIZZLE_MAX_PROCESSES]) }
    }

    /// Retrieves a mutable reference to the data held within this IPC memory.
    pub fn data(&mut self) -> &mut InterprocessState {
        unsafe {
            &mut *((self.mem_start as *mut libc::sem_t).add(FIZZLE_MAX_PROCESSES)
                as *mut InterprocessState)
        }
    }

    /// Wakes up the process designated by `process_id`.
    pub fn process_wake(&mut self, process_id: ProcessId) {
        let proc_id: usize = process_id.into();
        assert!(
            proc_id < FIZZLE_MAX_PROCESSES,
            "internal fizzle process_wake function called with invalid ProcessId"
        );

        unsafe { libc::sem_post(ptr::addr_of_mut!(self.process_locks()[proc_id])) };
    }

    /// Waits for the lock associated with `process_id` to be unlocked.
    pub fn process_wait(&mut self, process_id: ProcessId) {
        let proc_id: usize = process_id.into();
        assert!(
            proc_id < FIZZLE_MAX_PROCESSES,
            "internal fizzle process_wait function called with invalid ProcessId"
        );

        while unsafe { libc::sem_wait(ptr::addr_of_mut!(self.process_locks()[proc_id])) } != 0 {}
    }

    #[allow(unused)]
    pub fn destroy(mut self) {
        let ret = unsafe {
            libc::munmap(
                ptr::addr_of_mut!(self.process_locks()[0]) as *mut libc::c_void,
                Self::SHMEM_LENGTH,
            )
        };
        debug_assert!(ret == 0, "`munmap` failed while destroying IpcMemory");
        let ret = unsafe { libc::close(self.memfd) };
        debug_assert!(ret == 0, "`close` failed while destroying IpcMemory");
    }
}
