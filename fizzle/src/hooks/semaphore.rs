use crate::state::{SemaphoreId, SemaphoreInfo};
use crate::{hook_macros, scheduler, state};

use std::collections::VecDeque;
use std::ffi::CStr;
use std::ptr;
use std::thread;



hook_macros::hook! {
    unsafe fn sem_init(
        sem: *mut libc::sem_t,
        _pshared: libc::c_int,
        value: libc::c_uint
    ) -> libc::c_int => fizzle_sem_init {

        // TODO: what about semaphores shared across processes?

        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        if state.semaphores.insert(semaphore_id, SemaphoreInfo {
            name: None,
            value: value as usize,
            waiting: VecDeque::new(),
        }).is_some() {
            crate::abort("`sem_init` called twice on one semaphore");
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
    ) -> *mut libc::sem_t => fizzle_sem_open {
        let mut state = state::fizzle_state().lock().unwrap();

        let name = CStr::from_ptr(name).to_owned();
        // TODO: validate name?

        if (oflag & libc::O_CREAT) != 0 {
            if (oflag & libc::O_EXCL) != 0 && state.named_semaphores.contains_key(&name) {
                *libc::__errno_location() = libc::EEXIST;
                return ptr::null_mut()
            }

            // TODO: we ignore `mode` file permissions here

            match state.named_semaphores.entry(name.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => return o.get().to_mut_ptr(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    let sem = crate::unique_mem_create() as *mut libc::sem_t;
                    let semaphore_id = SemaphoreId::from(sem);

                    v.insert(semaphore_id);
                    state.semaphores.insert(semaphore_id, SemaphoreInfo {
                        name: Some(name),
                        value: value as usize,
                        waiting: VecDeque::new(),
                    });
                    return sem
                },
            }
        } else { // Open existing semaphore
            let Some(semaphore_id) = state.named_semaphores.get(&name) else {
                *libc::__errno_location() = libc::ENOENT;
                return ptr::null_mut()
            };

            semaphore_id.to_mut_ptr()
        }
    }
}

hook_macros::hook! {
    unsafe fn sem_destroy(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_destroy {
        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        let Some(semaphore) = state.semaphores.remove(&semaphore_id) else {
            crate::abort("`sem_destroy` called on uninitialized semaphore");
        };
        
        if !semaphore.waiting.is_empty() {
            crate::abort("`sem_destroy` called on semaphore while threads were waiting on it");
        }

        if semaphore.name.is_some() {
            crate::abort("`sem_destroy` called on named semaphore (should be `sem_close`");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_close(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_close {
        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        // TODO: this shouldn't be remove for multi-process applications...
        let Some(semaphore) = state.semaphores.remove(&semaphore_id) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1;
        };

        let Some(name) = semaphore.name else {
            crate::abort("`sem_close` called on unnamed semaphore (should be `sem_destroy`");
        };

        if !semaphore.waiting.is_empty() {
            crate::abort("`sem_close` called on semaphore while threads were waiting on it");
        }

        if state.named_semaphores.remove(&name).is_none() {
            crate::abort("inconsistent internal state (named_semaphore missing name)");
        }

        crate::unique_mem_destroy(semaphore_id.to_mut_ptr() as *mut libc::c_void);

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_unlink(
        _sem: *const libc::c_char
    ) -> libc::c_int => fizzle_sem_unlink {

        crate::debug_abort("sem_unlink");

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_post(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_post {
        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        let Some(semaphore) = state.semaphores.get_mut(&semaphore_id) else {
            crate::abort("`sem_post` called on uninitialized semaphore");
        };

        match semaphore.waiting.pop_front() {
            Some(waiting_thread) => state.ready_threads.push_back(waiting_thread),
            None => semaphore.value += 1,
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_wait(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_wait {
        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        let Some(semaphore) = state.semaphores.get_mut(&semaphore_id) else {
            crate::abort("`sem_wait` called on uninitialized semaphore");
        };

        match semaphore.value.checked_sub(1) {
            Some(value) => semaphore.value = value,
            None => {
                semaphore.waiting.push_back(thread::current().id());
                drop(state);
                scheduler::yield_thread();
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn sem_trywait(
        sem: *mut libc::sem_t
    ) -> libc::c_int => fizzle_sem_trywait {
        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        let Some(semaphore) = state.semaphores.get_mut(&semaphore_id) else {
            crate::abort("`sem_trywait` called on uninitialized semaphore");
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
    ) -> libc::c_int => fizzle_sem_timedwait {
        let mut state = state::fizzle_state().lock().unwrap();
        let semaphore_id = SemaphoreId::from(sem);

        let Some(semaphore) = state.semaphores.get_mut(&semaphore_id) else {
            crate::abort("`sem_timedwait` called on uninitialized semaphore");
        };

        match semaphore.value.checked_sub(1) {
            Some(value) => semaphore.value = value,
            None => {
                semaphore.waiting.push_back(thread::current().id());
                drop(state);
                scheduler::yield_thread();
            },
        }

        0
    }
}
