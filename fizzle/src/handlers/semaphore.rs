use std::collections::VecDeque;
use std::ffi::CStr;
use std::fmt::Display;

use bitflags::bitflags;
use fizzle_common::path::SemaphorePath;

use crate::arena::ArenaKey;
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;
use crate::WaitDuration;

pub use private::SemaphoreId;

use super::file::AccessMode;
use super::id::Worker;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct SemaphoreId(usize);
}

impl ArenaKey for SemaphoreId {
    type Value = SemaphoreInfo;
}

pub struct SemaphoreInfo {
    pub refs: usize,
    pub unlinked: bool,
    pub value: usize,
    pub waiting: VecDeque<Worker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemaphorePtr(usize);

impl From<*mut libc::sem_t> for SemaphorePtr {
    fn from(value: *mut libc::sem_t) -> Self {
        SemaphorePtr(value as usize)
    }
}

impl SemaphorePtr {
    pub unsafe fn to_mut_ptr(self) -> *mut libc::sem_t {
        self.0 as *mut libc::sem_t // TODO: breaks provenance guarantees
    }
}

impl SemaphoreId {}

pub struct SemInitEvent {
    sem: SemaphorePtr,
    pshared: bool,
    value: u32,
}

impl SemInitEvent {
    pub fn new(sem: SemaphorePtr, pshared: bool, value: u32) -> Self {
        Self {
            sem,
            pshared,
            value,
        }
    }
}

impl Event for SemInitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if self.pshared {
            panic!("shared anonymous semaphores unsupported by fizzle")
        }

        if state
            .local
            .semaphores
            .insert(
                self.sem,
                SemaphoreInfo {
                    refs: 1,         // Unused except for named semaphores
                    unlinked: false, // Unused except for named semaphores
                    value: self.value as usize,
                    waiting: VecDeque::new(),
                },
            )
            .is_some()
        {
            log::warn!("`sem_init` called twice on one semaphore");
        }

        Outcome::Success(())
    }
}

bitflags! {
    #[derive(Debug)]
    pub struct SemOpenFlags: libc::c_int {
        const CREATE = libc::O_CREAT;
        const EXCLUSIVE = libc::O_EXCL;
    }
}

impl Display for SemOpenFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.contains(Self::CREATE) {
            f.write_str("O_CREAT")?;
        }

        if self.is_all() {
            f.write_str("|")?;
        }

        if self.contains(Self::EXCLUSIVE) {
            f.write_str("O_EXCL")?;
        }

        Ok(())
    }
}

pub struct SemOpenEvent<'a> {
    name: &'a CStr,
    exclusive: bool,
    create: Option<(AccessMode, u32)>,
}

impl<'a> SemOpenEvent<'a> {
    #[inline]
    pub fn new(name: &'a CStr, exclusive: bool, create: Option<(AccessMode, u32)>) -> Self {
        Self {
            name,
            exclusive,
            create,
        }
    }
}

impl Event for SemOpenEvent<'_> {
    type Success = SemaphorePtr;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Ok(sem_path) = SemaphorePath::from_cstr(self.name) else {
            return Outcome::Error(Errno::EINVAL);
        };

        if let Some((_access, value)) = self.create {
            // Create new semaphore

            if self.exclusive && state.global.sem_paths.contains_key(&sem_path) {
                return Outcome::Error(Errno::EEXIST);
            }

            // TODO: we ignore access `mode` permissions here

            let sem = unsafe { crate::unique_mem_create() } as *mut libc::sem_t;
            let semaphore_ptr = SemaphorePtr::from(sem);

            let sem_id = state
                .global
                .semaphores
                .allocate(SemaphoreInfo {
                    refs: 1,
                    unlinked: false,
                    value: value as usize,
                    waiting: VecDeque::new(),
                })
                .unwrap();

            state.local.named_semaphores.insert(semaphore_ptr, sem_id);

            Outcome::Success(semaphore_ptr)
        } else if let Some(sem_id) = state.global.sem_paths.get(&sem_path).cloned() {
            // Open existing semaphore

            let sem = unsafe { crate::unique_mem_create() } as *mut libc::sem_t;
            let semaphore_ptr = SemaphorePtr::from(sem);

            state
                .local
                .named_semaphores
                .insert(semaphore_ptr, sem_id.clone())
                .unwrap();

            let sem_ctx = state.global.semaphores.get_mut(&sem_id).unwrap();
            sem_ctx.refs += 1;

            Outcome::Success(semaphore_ptr)
        } else {
            // No existing semaphore
            Outcome::Error(Errno::ENOENT)
        }
    }
}

pub struct SemDestroyEvent {
    sem: SemaphorePtr,
}

impl SemDestroyEvent {
    #[inline]
    pub fn new(sem: SemaphorePtr) -> Self {
        Self { sem }
    }
}

impl Event for SemDestroyEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if state.local.named_semaphores.contains_key(&self.sem) {
            log::warn!("`sem_destroy` called on named pointer");
            return Outcome::Error(Errno::EINVAL);
        }

        let Some(semaphore) = state.local.semaphores.remove(&self.sem) else {
            log::warn!("`sem_destroy` called on uninitialized semaphore");
            return Outcome::Error(Errno::EINVAL);
        };

        unsafe {
            crate::unique_mem_destroy(self.sem.to_mut_ptr().cast::<libc::c_void>());
        }

        if !semaphore.waiting.is_empty() {
            panic!("[UB] `sem_destroy` called on semaphore while threads were still waiting on it")
        }

        Outcome::Success(())
    }
}

pub struct SemCloseEvent {
    sem: SemaphorePtr,
}

impl SemCloseEvent {
    pub fn new(sem: SemaphorePtr) -> Self {
        Self { sem }
    }
}

impl Event for SemCloseEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(sem_id) = state.local.named_semaphores.remove(&self.sem) else {
            return Outcome::Error(Errno::EINVAL);
        };

        unsafe {
            crate::unique_mem_destroy(self.sem.to_mut_ptr().cast::<libc::c_void>());
        }

        let Some(sem_ctx) = state.global.semaphores.get_mut(&sem_id) else {
            panic!("inconsistent fizzle state--named semaphore without global context in `sem_close()`");
        };

        sem_ctx.refs -= 1;
        if sem_ctx.refs == 0 && sem_ctx.unlinked {
            state.global.semaphores.downref(&sem_id);
        }

        return Outcome::Success(());
    }
}

pub struct SemUnlinkEvent<'a> {
    path: &'a CStr,
}

impl<'a> SemUnlinkEvent<'a> {
    pub fn new(path: &'a CStr) -> Self {
        Self { path }
    }
}

impl Event for SemUnlinkEvent<'_> {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Ok(sem_path) = SemaphorePath::from_cstr(self.path) else {
            return Outcome::Error(Errno::EINVAL);
        };

        let Some(sem_id) = state.global.sem_paths.remove(&sem_path) else {
            log::warn!("`sem_unlink` called on nonexistent named semaphore");
            return Outcome::Error(Errno::ENOENT);
        };

        let Some(sem_info) = state.global.semaphores.get_mut(&sem_id) else {
            panic!("inconsistent Fizzle state--named semaphore without global context in `sem_unlink()`")
        };

        sem_info.unlinked = true;
        if sem_info.refs == 0 {
            assert!(sem_info.waiting.is_empty(), "inconsistent Fizzle state--named semaphore wait queue not empty after `sem_unlink()`");
        } else {
            state.global.semaphores.upref(&sem_id);
        }

        Outcome::Success(())
    }
}

pub struct SemPostEvent {
    sem: SemaphorePtr,
}

impl<'a> SemPostEvent {
    pub fn new(sem: SemaphorePtr) -> Self {
        Self { sem }
    }
}

impl Event for SemPostEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if let Some(sem_info) = state.local.semaphores.get_mut(&self.sem) {
            match sem_info.waiting.pop_front() {
                Some(worker_id) => state.mark_thread_ready(worker_id.thread_id),
                None => sem_info.value += 1,
            }

            Outcome::Success(())
        } else if let Some(semaphore_id) = state.local.named_semaphores.get(&self.sem).cloned() {
            let Some(sem_info) = state.global.semaphores.get_mut(&semaphore_id) else {
                panic!("inconsistent fizzle state--named semaphore without global context in `sem_post()`");
            };

            match sem_info.waiting.pop_front() {
                Some(worker_id) => state.global.mark_worker_ready(worker_id),
                None => sem_info.value += 1,
            }

            Outcome::Success(())
        } else {
            log::warn!("`sem_post()` passed in invalid semaphore pointer");
            Outcome::Error(Errno::EINVAL)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SemWaitState {
    Start,
    Finish,
}

pub struct SemWaitEvent {
    sem: SemaphorePtr,
    duration: WaitDuration,
    state: SemWaitState,
}

impl SemWaitEvent {
    pub fn new(sem: SemaphorePtr, duration: WaitDuration) -> Self {
        Self {
            sem,
            duration,
            state: SemWaitState::Start,
        }
    }
}

impl Event for SemWaitEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_worker_id = state.current_worker_id();

        match self.state {
            SemWaitState::Start => {
                let Some(semaphore) = state.local.semaphores.get_mut(&self.sem) else {
                    log::warn!("semaphore to wait on was uninitialized");
                    return Outcome::Error(Errno::EINVAL);
                };

                match semaphore.value.checked_sub(1) {
                    Some(value) => {
                        semaphore.value = value;
                        Outcome::Success(())
                    }
                    None => match self.duration {
                        WaitDuration::Immediate => Outcome::Error(Errno::EAGAIN),
                        WaitDuration::Timed(t) => {
                            semaphore.waiting.push_back(current_worker_id);
                            self.state = SemWaitState::Finish;
                            Outcome::Yield(Some(t))
                        }
                        WaitDuration::Indefinite => {
                            semaphore.waiting.push_back(current_worker_id);
                            self.state = SemWaitState::Finish;
                            Outcome::Yield(None)
                        }
                    },
                }
            }
            SemWaitState::Finish => {
                let Some(semaphore) = state.local.semaphores.get_mut(&self.sem) else {
                    panic!("[UB] semaphore being waited on by sem_wait was destroyed");
                };

                // Check to see if this is due to timeout
                for (idx, worker_id) in semaphore.waiting.iter().enumerate() {
                    if worker_id == &current_worker_id {
                        // Remove the current worker from the wait queue
                        semaphore.waiting.remove(idx).unwrap();

                        // The worker was still waiting--this must have been a timeout wakeup
                        let WaitDuration::Timed(t) = self.duration else {
                            panic!("internal Fizzle error: semaphore awakened despite worker still being in queue");
                        };

                        log::debug!("sem_timedwait() timed out after {:?}", t);
                        return Outcome::Error(Errno::ETIMEDOUT);
                    }
                }

                // The worker was dequeued from the semaphore--ready to run
                Outcome::Success(())
            }
        }
    }
}
