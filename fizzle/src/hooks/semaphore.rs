use heapless::Deque;

use crate::state::identifiers::{SemaphorePtr, WorkerId};
use crate::state::SemaphoreInfo;
use crate::{hook_macros, state};

use fizzle_common::path::SemPath;

use std::ffi::CStr;
use std::ptr;
use std::thread;

hook_macros::hook! {
    unsafe fn sem_init(
        sem: *mut libc::sem_t,
        pshared: libc::c_int,
        value: libc::c_uint
    ) -> libc::c_int => fizzle_sem_init(ctx) {

        // TODO: what about semaphores shared via memory across processes?

        if pshared != 0 {
            panic!("shared anonymous semaphores unsupported by fizzle")
        }

        let semaphore_id = SemaphorePtr::from(sem);

        if ctx.local.semaphores.insert(semaphore_id, SemaphoreInfo {
            refs: 1, // Unused except for named semaphores
            unlinked: false, // Unused except for named semaphores
            value: value as usize,
            waiting: Deque::new(),
        }).is_some() {
            log::warn!("`sem_init` called twice on one semaphore");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_open(
        name: *const libc::c_char,
        oflag: libc::c_int,
        _mode: libc::mode_t,
        value: libc::c_uint
    ) -> *mut libc::sem_t => fizzle_sem_open(ctx) {

        let name = CStr::from_ptr(name);
        let Ok(sem_path) = SemPath::from_cstr(name) else {
            *libc::__errno_location() = libc::EINVAL;
            return ptr::null_mut()
        };

        if (oflag & libc::O_CREAT) != 0 {
            if (oflag & libc::O_EXCL) != 0 && ctx.global.sem_paths.contains_key(&sem_path) {
                *libc::__errno_location() = libc::EEXIST;
                return ptr::null_mut()
            }

            // TODO: we ignore `mode` file permissions here

            let sem = crate::unique_mem_create() as *mut libc::sem_t;
            let semaphore_ptr = SemaphorePtr::from(sem);

            let sem_id = ctx.global.semaphores.allocate(SemaphoreInfo {
                refs: 1,
                unlinked: false,
                value: value as usize,
                waiting: Deque::new(),
            }).unwrap();

            ctx.local.named_semaphores.insert(semaphore_ptr, sem_id);

            semaphore_ptr.to_mut_ptr()

        } else { // Open existing semaphore
            if let Some(sem_id) = ctx.global.sem_paths.get(&sem_path).cloned() {
                let sem = crate::unique_mem_create() as *mut libc::sem_t;
                let semaphore_ptr = SemaphorePtr::from(sem);

                ctx.local.named_semaphores.insert(semaphore_ptr, sem_id.clone()).unwrap();

                let sem_ctx = ctx.global.semaphores.get_mut(&sem_id).unwrap();
                sem_ctx.refs += 1;

                sem
            } else {
                *libc::__errno_location() = libc::ENOENT; // TODO: check validity
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_destroy(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_destroy(ctx) {

        let sem_ptr = SemaphorePtr::from(sem);

        if ctx.local.named_semaphores.contains_key(&sem_ptr) {
            log::warn!("`sem_destroy` called on named pointer");
            *libc::__errno_location() = libc::EINVAL;
            return -1
        }

        let Some(semaphore) = ctx.local.semaphores.remove(&sem_ptr) else {
            log::warn!("`sem_destroy` called on uninitialized semaphore");
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };
        crate::unique_mem_destroy(sem as *mut libc::c_void);

        if !semaphore.waiting.is_empty() {
            panic!("[UB] `sem_destroy` called on semaphore while threads were still waiting on it")
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_close(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_close(ctx) {

        let sem_ptr = SemaphorePtr::from(sem);

        let Some(sem_id) = ctx.local.named_semaphores.remove(&sem_ptr) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1;
        };

        crate::unique_mem_destroy(sem as *mut libc::c_void); // TODO: make sure this is called everywhere
        let Some(sem_ctx) = ctx.global.semaphores.get_mut(&sem_id) else {
            panic!("inconsistent fizzle state--named semaphore without global context in `sem_close`");
        };

        sem_ctx.refs -= 1;
        if sem_ctx.refs == 0 && sem_ctx.unlinked {
            ctx.global.semaphores.downref(&sem_id);
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_unlink(
        sem: *const libc::c_char
    ) -> libc::c_int => fizzle_sem_unlink(ctx) {

        let name = CStr::from_ptr(sem);
        let Ok(sem_path) = SemPath::from_cstr(name) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(sem_id) = ctx.global.sem_paths.remove(&sem_path) else {
            log::debug!("`sem_unlink` called on nonexistent named semaphore");
            *libc::__errno_location() = libc::ENOENT;
            return -1
        };

        let Some(sem_info) = ctx.global.semaphores.get_mut(&sem_id) else {
            panic!("inconsistent fizzle state--named semaphore without global context in `sem_unlink`")
        };

        sem_info.unlinked = true;
        if sem_info.refs == 0 {
            assert!(sem_info.waiting.is_empty(), "inconsistent fizzle state--global sem waiting queue not empty when refs are zero");
            // sem_id dropped => semaphore destroyed
        } else {
            ctx.global.semaphores.upref(&sem_id);
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_post(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_post(ctx) {

        let semaphore_ptr = SemaphorePtr::from(sem);
        if let Some(sem_info) = ctx.local.semaphores.get_mut(&semaphore_ptr) {
            match sem_info.waiting.pop_front() {
                Some(worker_id) => ctx.mark_thread_ready(worker_id.thread_id),
                None => sem_info.value += 1,
            }

        } else if let Some(semaphore_id) = ctx.local.named_semaphores.get(&semaphore_ptr).cloned() {
            let Some(sem_info) = ctx.global.semaphores.get_mut(&semaphore_id) else {
                panic!("inconsistent fizzle state--named semaphore without global context in `sem_unlink`");
            };

            match sem_info.waiting.pop_front() {
                Some(worker_id) => ctx.global.mark_worker_ready(worker_id),
                None => sem_info.value += 1,
            }

        } else {
            log::debug!("`sem_post` passed in invalid semaphore pointer");
            *libc::__errno_location() = libc::EINVAL;
            return -1
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_wait(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_wait(ctx) {
        let semaphore_id = SemaphorePtr::from(sem);
        let process_id = ctx.local.process_id;

        let Some(semaphore) = ctx.local.semaphores.get_mut(&semaphore_id) else {
            log::debug!("`sem_wait` called on uninitialized semaphore");
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        match semaphore.value.checked_sub(1) {
            Some(value) => semaphore.value = value,
            None => {
                semaphore.waiting.push_back(WorkerId {
                    process_id,
                    thread_id: thread::current().id()
                }).unwrap();
                drop(ctx);
                state::FIZZLE_STATE.yield_thread();
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_trywait(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_trywait(ctx) {
        let semaphore_id = SemaphorePtr::from(sem);

        let Some(semaphore) = ctx.local.semaphores.get_mut(&semaphore_id) else {
            log::debug!("`sem_trywait` called on uninitialized semaphore");
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        match semaphore.value.checked_sub(1) {
            Some(value) => semaphore.value = value,
            None => {
                *libc::__errno_location() = libc::EAGAIN;
                return -1
            },
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_timedwait(
        sem: *mut libc::sem_t,
        _abs_timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_sem_timedwait(ctx) {
        let semaphore_id = SemaphorePtr::from(sem);
        let process_id = ctx.local.process_id;

        let Some(semaphore) = ctx.local.semaphores.get_mut(&semaphore_id) else {
            log::debug!("`sem_timedwait` called on uninitialized semaphore");
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        match semaphore.value.checked_sub(1) {
            Some(value) => semaphore.value = value,
            None => {
                semaphore.waiting.push_back(WorkerId {
                    process_id,
                    thread_id: thread::current().id()
                }).unwrap();
                drop(ctx);
                state::FIZZLE_STATE.yield_thread();
            }
        }

        0
    }
}
