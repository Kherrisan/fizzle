use crate::state::{BarrierInfo, CondVarId, BarrierId, MutexId, RwLockId, RwLockInfo, RwLockState, SpinlockId, ThreadInfo};
use crate::{hook_macros, scheduler, state};

use std::collections::{HashSet, VecDeque};
use std::ptr;
use std::thread::{self, ThreadId};

type PTFunction = unsafe extern "C" fn(*mut libc::c_void) -> *mut libc::c_void;

#[repr(C)]
struct PTWrapperArgs {
    wrapped_fn: PTFunction,
    wrapped_arg: *mut libc::c_void,
}

unsafe extern "C" fn pt_wrapper_fn(arg: *mut libc::c_void) -> *mut libc::c_void {
    crate::trace_enter!("pt_wrapper_fn");

    let wrapped_arg = (arg as *mut PTWrapperArgs).as_mut().unwrap();

    // Before we do ANYTHING, we need to set this to avoid accidental preload hook recursion
    state::set_entered_handler(true);

    let mut state = state::fizzle_state().lock().unwrap();
    let current_thread = thread::current();
    state.program_threads.insert(current_thread.id(), ThreadInfo {
        thread: current_thread,
        delegated: false,
    });

    state.pthreads.insert(unsafe { libc::pthread_self() }, thread::current().id());

    drop(state);

    // Now enable preload hooks to actually work during this thread's execution
    state::set_entered_handler(false);

    let res = (wrapped_arg.wrapped_fn)(wrapped_arg.wrapped_arg);
    // Thread has exited...

    // Once again, avoid accidental preload hook recursion
    state::set_entered_handler(true);
    
    // Flag this thread as dying so that scheduler can clean its context up
    let mut state = state::fizzle_state().lock().unwrap();
    state.exit_current_thread();

    // Pass control flow back to the scheduler
    state.wake_next_thread();
    drop(state);

    crate::trace_exit!("pt_wrapper_fn");
    // Go out of scope (happens concurrently)
    res
}

hook_macros::hook! {
    unsafe fn pthread_create(
        thread: *mut libc::pthread_t,
        attr: *const libc::pthread_attr_t,
        start_routine: PTFunction,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_pthread_create {

        let mut wrapped_arg = PTWrapperArgs {
            wrapped_fn: start_routine,
            wrapped_arg: arg,
        };

        let mut state = state::fizzle_state().lock().unwrap();

        state.ready_threads.push_back(thread::current().id()); // Let the scheduler know we have more to execute
        drop(state);

        let res = hook_macros::real!(pthread_create)(thread, attr, pt_wrapper_fn, ptr::addr_of_mut!(wrapped_arg) as *mut libc::c_void);

        // The newly-created thread executes now, so this thread pauses
        
        thread::park();
        while !state::fizzle_state().lock().unwrap().thread_delegated() {
            thread::park();
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn pthread_exit(
        retval: *mut libc::c_void
    ) => fizzle_pthread_exit {
        
        // Flag this thread as dying so that scheduler can clean its context up
        let mut state = state::fizzle_state().lock().unwrap();
        state.exit_current_thread();

        // Pass control flow to the next ready thread
        state.wake_next_thread();
        drop(state);

        // Go out of scope (happens concurrently)
        hook_macros::real!(pthread_exit)(retval);
    }
}

// TODO: pthread_join
// Save pthread_t values of each thread
// When a given thread is going to exit, check to see if any pthread_join() calls are waiting on it
// If they are, then mark those threads as active before exiting the thread

hook_macros::hook! {
    unsafe fn pthread_join(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void
    ) => fizzle_pthread_join {
        let mut state = state::fizzle_state().lock().unwrap();

        let target_id = state.pthreads.remove(&thread).unwrap();
        if !state.terminated_threads.contains(&target_id) {
            // Target thread has not yet terminated--add it to list of threads awaiting death of target
            match state.awaiting_thread_death.entry(target_id) {
                std::collections::hash_map::Entry::Occupied(mut o) => o.get_mut().push(thread::current().id()),
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(vec![ thread::current().id() ]);
                }
            }
            drop(state);

            scheduler::yield_thread();
        }
        // Waiting thread has now terminated--join properly

        hook_macros::real!(pthread_join)(thread, retval);
    }
}


// TODO: pthread_cancel
// Save pthread_t and pthread_setcancel_state values
// When a thread tries to cancel another, check cancel state. If thread is cancellable, set a
// variable that indicates to the scheduler that it should shut down that thread.
// TODO: deferred cancellation as well--have to hook all known cancellation points and go from there
// TODO: handle cancellation cleanup handlers


hook_macros::hook! {
    unsafe fn pthread_cancel(
        thread: libc::pthread_t
    ) => fizzle_pthread_cancel {

        crate::debug_abort("pthread_cancel");

        hook_macros::real!(pthread_cancel)(thread);
    }
}

hook_macros::hook! {
    unsafe fn pthread_tryjoin_np(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void
    ) -> libc::c_int => fizzle_pthread_tryjoin_np {
        hook_macros::real!(pthread_tryjoin_np)(thread, retval)
    }
}

hook_macros::hook! {
    unsafe fn pthread_timedjoin_np(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_timedjoin_np {

        crate::debug_abort("pthread_timedjoin_np");

        hook_macros::real!(pthread_timedjoin_np)(thread, retval, abstime)
    }
}

hook_macros::hook! {
    unsafe fn pthread_clockjoin_np(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void,
        clock_id: *mut libc::clockid_t,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_clockjoin_np {

        crate::debug_abort("pthread_timedjoin_np");

        hook_macros::real!(pthread_clockjoin_np)(thread, retval, clock_id, abstime)
    }
}


hook_macros::hook! {
    unsafe fn pthread_detach(
        thread: libc::pthread_t
    ) => fizzle_pthread_detach {

        crate::debug_abort("pthread_detach");

        hook_macros::real!(pthread_detach)(thread);
    }
}

hook_macros::hook! {
    unsafe fn pthread_kill(
        thread: libc::pthread_t,
        sig: libc::c_int
    ) -> libc::c_int => fizzle_pthread_kill {

        crate::debug_abort("pthread_kill");

        hook_macros::real!(pthread_kill)(thread, sig)
    }
}

hook_macros::hook! {
    unsafe fn pthread_yield(
    ) -> libc::c_int => fizzle_pthread_yield {

        let mut state = state::fizzle_state().lock().unwrap();
        state.ready_threads.push_back(thread::current().id());
        drop(state);

        scheduler::yield_thread();

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_setcancelstate(
        state: libc::c_int,
        old_state: *mut libc::c_int
    ) -> libc::c_int => fizzle_pthread_setcancelstate {

        crate::debug_abort("pthread_setcancelstate");

        hook_macros::real!(pthread_setcancelstate)(state, old_state)
    }
}

hook_macros::hook! {
    unsafe fn pthread_setcanceltype(
        cancel_type: libc::c_int,
        old_type: *mut libc::c_int
    ) -> libc::c_int => fizzle_pthread_setcanceltype {

        crate::debug_abort("pthread_setcanceltype");

        hook_macros::real!(pthread_setcancelstate)(cancel_type, old_type)
    }
}

hook_macros::hook! {
    unsafe fn pthread_testcancel(
    ) -> libc::c_int => fizzle_pthread_testcancel {

        crate::debug_abort("pthread_testcancel");

        hook_macros::real!(pthread_testcancel)()
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_init(
        lock: *mut libc::pthread_spinlock_t,
        _shared: libc::c_int
    ) -> libc::c_int => fizzle_pthread_spin_init {

        // TODO: what about mutexes shared across processes?

        let mut state = state::fizzle_state().lock().unwrap();
        let spinlock = SpinlockId::from(lock);

        if state.spinlocks.insert(spinlock, VecDeque::new()).is_some() {
            crate::abort("`pthread_spin_init` called twice on one spinlock");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_destroy(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_destroy {

        let mut state = state::fizzle_state().lock().unwrap();
        let spinlock = SpinlockId::from(lock);

        let Some(spinlock_queue) = state.spinlocks.remove(&spinlock) else {
            crate::abort("`pthread_spin_destroy` called on uninitialized spinlock")
        };

        if !spinlock_queue.is_empty() {
            crate::abort("`pthread_spin_destroy` called on locked spinlock") // Undefined behavior
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_lock(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_lock {

        let mut state = state::fizzle_state().lock().unwrap();
        let spinlock = SpinlockId::from(lock);

        let Some(spinlock_queue) = state.spinlocks.get_mut(&spinlock) else {
            crate::abort("`pthread_spin_lock` called on uninitialized spinlock")
        };

        let available = spinlock_queue.is_empty();
        spinlock_queue.push_back(thread::current().id());

        if !available {
            drop(state);
            scheduler::yield_thread();
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_trylock(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_trylock {

        let mut state = state::fizzle_state().lock().unwrap();
        let spinlock = SpinlockId::from(lock);

        let Some(spinlock_queue) = state.spinlocks.get_mut(&spinlock) else {
            crate::abort("`pthread_spin_trylock` called on uninitialized spinlock")
        };

        if !spinlock_queue.is_empty() {
            return libc::EBUSY
        }
        spinlock_queue.push_back(thread::current().id());

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_unlock(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_unlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let spinlock = SpinlockId::from(lock);

        let Some(spinlock_queue) = state.spinlocks.get_mut(&spinlock) else {
            crate::abort("`pthread_spin_unlock` called on uninitialized spinlock")
        };

        let Some(popped_thread) = spinlock_queue.pop_front() else {
            crate::abort("`pthread_spin_unlock` called when spinlock already unlocked")
        };

        if popped_thread != thread::current().id() {
            crate::abort("`pthread_spin_unlock` called by a thread not currently holding the spinlock")
        }

        if let Some(next_thread) = spinlock_queue.front().map(|t| t.clone()) {
            state.ready_threads.push_back(next_thread);
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_init(
        lock: *mut libc::pthread_mutex_t,
        _attr: *mut libc::pthread_mutexattr_t
    ) -> libc::c_int => fizzle_pthread_mutex_init {

        // TODO: what about mutexes shared across processes?

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        if state.mutexes.insert(mutex, VecDeque::new()).is_some() {
            crate::abort("`pthread_mutex_init` called twice on one mutex");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_destroy(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_destroy {

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        let Some(mutex_queue) = state.mutexes.remove(&mutex) else {
            crate::abort("`pthread_mutex_destroy` called on uninitialized mutex")
        };

        if !mutex_queue.is_empty() {
            crate::abort("`pthread_mutex_destroy` called on locked mutex")
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_lock(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_lock {

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_lock` called on uninitialized mutex")
        };

        let available = mutex_queue.is_empty();
        mutex_queue.push_back(thread::current().id());

        if !available {
            drop(state);
            scheduler::yield_thread();
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_trylock(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_trylock {

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_trylock` called on uninitialized mutex")
        };

        if !mutex_queue.is_empty() {
            return libc::EBUSY
        }
        mutex_queue.push_back(thread::current().id());

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_timedlock(
        lock: *mut libc::pthread_mutex_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_mutex_timedlock {
        // TODO: what about mutexes shared across processes?

        // TODO: this just returns immediately if locked

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_timedlock` called on uninitialized mutex")
        };

        if !mutex_queue.is_empty() {
            return libc::EBUSY
        }
        mutex_queue.push_back(thread::current().id());

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_clocklock(
        lock: *mut libc::pthread_mutex_t,
        _clock_id: libc::clockid_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_mutex_clocklock {
        // TODO: what about mutexes shared across processes?

        // TODO: this just returns immediately if locked

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_clocklock` called on uninitialized mutex")
        };

        if !mutex_queue.is_empty() {
            return libc::EBUSY
        }
        mutex_queue.push_back(thread::current().id());

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_unlock(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_unlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let mutex = MutexId::from(lock);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_unlock` called on uninitialized mutex")
        };

        let Some(popped_thread) = mutex_queue.pop_front() else {
            crate::abort("`pthread_mutex_unlock` called when mutex already unlocked")
        };

        if popped_thread != thread::current().id() {
            crate::abort("`pthread_mutex_unlock` called by a thread not currently holding the lock")
        }

        if let Some(next_thread) = mutex_queue.front().map(|t| t.clone()) {
            state.ready_threads.push_back(next_thread);
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_consistent(
        _mutex: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_consistent {

        // TODO: make poisoned lock behavior compliant with POSIX

        crate::abort("Unimplemented `pthread_mutex_consistent`");
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_init(
        lock: *mut libc::pthread_cond_t,
        _attr: *mut libc::pthread_condattr_t
    ) -> libc::c_int => fizzle_pthread_cond_init {

        // TODO: what about mutexes shared across processes?

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        if state.condvars.insert(cond, VecDeque::new()).is_some() {
            crate::abort("`pthread_cond_init` called twice on one condvar");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_destroy(
        lock: *mut libc::pthread_cond_t
    ) -> libc::c_int => fizzle_pthread_cond_destroy {

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        let Some(condvar_queue) = state.condvars.remove(&cond) else {
            crate::abort("`pthread_cond_destroy` called on uninitialized condvar")
        };

        if !condvar_queue.is_empty() {
            crate::abort("`pthread_cond_destroy` called on locked condvar") // Undefined behavior
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_signal(
        lock: *mut libc::pthread_cond_t
    ) -> libc::c_int => fizzle_pthread_cond_signal {

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        let Some(cond_queue) = state.condvars.get_mut(&cond) else {
            crate::abort("`pthread_cond_signal` called on uninitialized condvar")
        };

        if let Some(thread) = cond_queue.pop_front() {
            state.ready_threads.push_back(thread);
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_broadcast(
        lock: *mut libc::pthread_cond_t
    ) -> libc::c_int => fizzle_pthread_cond_broadcast {

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        let Some(cond_queue) = state.condvars.get_mut(&cond) else {
            crate::abort("`pthread_cond_broadcast` called on uninitialized condvar")
        };

        let threads: Vec<ThreadId> = cond_queue.drain(..).collect();
        
        for thread in threads {
            state.ready_threads.push_back(thread);
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_wait(
        lock: *mut libc::pthread_cond_t,
        mutex: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_cond_wait {

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        let Some(cond_queue) = state.condvars.get_mut(&cond) else {
            crate::abort("`pthread_cond_wait` called on uninitialized condvar")
        };

        cond_queue.push_back(thread::current().id());

        // Now unlock the mutex
        let mutex = MutexId::from(mutex);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_cond_wait` called on uninitialized mutex")
        };

        let Some(popped_thread) = mutex_queue.pop_front() else {
            crate::abort("`pthread_cond_wait` called when mutex already unlocked")
        };

        if popped_thread != thread::current().id() {
            crate::abort("`pthread_cond_wait` called by a thread not currently holding the lock")
        }

        if let Some(next_thread) = mutex_queue.front().map(|t| t.clone()) {
            state.ready_threads.push_back(next_thread);
        }

        drop(state);

        // Wait until the thread is signaled
        scheduler::yield_thread();

        // Now re-lock the mutex
        let mut state = state::fizzle_state().lock().unwrap();

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_lock` called on uninitialized mutex")
        };

        let available = mutex_queue.is_empty();
        mutex_queue.push_back(thread::current().id());

        if !available {
            drop(state);
            scheduler::yield_thread();
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_timedwait(
        lock: *mut libc::pthread_cond_t,
        mutex: *mut libc::pthread_mutex_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_cond_timedwait {

        // TODO: timeout is infinite by default

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        let Some(cond_queue) = state.condvars.get_mut(&cond) else {
            crate::abort("`pthread_cond_wait` called on uninitialized condvar")
        };

        cond_queue.push_back(thread::current().id());

        // Now unlock the mutex
        let mutex = MutexId::from(mutex);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_cond_wait` called on uninitialized mutex")
        };

        let Some(popped_thread) = mutex_queue.pop_front() else {
            crate::abort("`pthread_cond_wait` called when mutex already unlocked")
        };

        if popped_thread != thread::current().id() {
            crate::abort("`pthread_cond_wait` called by a thread not currently holding the lock")
        }

        if let Some(next_thread) = mutex_queue.front().map(|t| t.clone()) {
            state.ready_threads.push_back(next_thread);
        }

        drop(state);

        // Wait until the thread is signaled
        scheduler::yield_thread();

        // Now re-lock the mutex
        let mut state = state::fizzle_state().lock().unwrap();

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_mutex_lock` called on uninitialized mutex")
        };

        let available = mutex_queue.is_empty();
        mutex_queue.push_back(thread::current().id());

        if !available {
            drop(state);
            scheduler::yield_thread();
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_clockwait(
        lock: *mut libc::pthread_cond_t,
        mutex: *mut libc::pthread_mutex_t,
        _clock_id: libc::clockid_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_cond_clockwait {

        // TODO: timeout is infinite by default

        let mut state = state::fizzle_state().lock().unwrap();
        let cond = CondVarId::from(lock);

        let Some(cond_queue) = state.condvars.get_mut(&cond) else {
            crate::abort("`pthread_cond_wait` called on uninitialized condvar")
        };

        cond_queue.push_back(thread::current().id());

        // Now unlock the mutex
        let mutex = MutexId::from(mutex);

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_cond_wait` called on uninitialized mutex")
        };

        let Some(popped_thread) = mutex_queue.pop_front() else {
            crate::abort("`pthread_cond_wait` called when mutex already unlocked")
        };

        if popped_thread != thread::current().id() {
            crate::abort("`pthread_cond_wait` called by a thread not currently holding the lock")
        }

        if let Some(next_thread) = mutex_queue.front().map(|t| t.clone()) {
            state.ready_threads.push_back(next_thread);
        }

        // Wait until the thread is signaled
        drop(state);
        scheduler::yield_thread();

        // Now re-lock the mutex
        let mut state = state::fizzle_state().lock().unwrap();

        let Some(mutex_queue) = state.mutexes.get_mut(&mutex) else {
            crate::abort("`pthread_cond_wait` mutex freed while waiting for condition")
        };

        let available = mutex_queue.is_empty();
        mutex_queue.push_back(thread::current().id());

        if !available {
            drop(state);
            scheduler::yield_thread();
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_init(
        lock: *mut libc::pthread_rwlock_t,
        _attr: *mut libc::pthread_rwlockattr_t
    ) -> libc::c_int => fizzle_pthread_rwlock_init {
        // TODO: what about mutexes shared across processes?

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        if state.rwlocks.insert(rwlock, RwLockInfo {
            state: RwLockState::Available,
            awaiting_read: VecDeque::new(),
            awaiting_write: VecDeque::new(),
            holding_state: HashSet::with_hasher(Default::default())
        }).is_some() {
            crate::abort("`pthread_rwlock_init` called twice on one rwlock");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_destroy(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_destroy {

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        let Some(rwlock_info) = state.rwlocks.remove(&rwlock) else {
            crate::abort("`pthread_rwlock_destroy` called on uninitialized rwlock")
        };

        if rwlock_info.state != RwLockState::Available {
            crate::abort("`pthread_rwlock_destroy` called on locked rwlock") // Undefined behavior
        }

        // Invariant: if RwLockState::Available, then neither of the Read/Write queues will have any threads
        assert!(rwlock_info.awaiting_read.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still awaiting read)");
        assert!(rwlock_info.awaiting_read.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still awaiting write)");
        assert!(rwlock_info.holding_state.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_rdlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_rdlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        let Some(rwlock_info) = state.rwlocks.get_mut(&rwlock) else {
            crate::abort("`pthread_rwlock_rdlock` called on uninitialized rwlock")
        };

        match rwlock_info.state {
            RwLockState::Writing => {
                rwlock_info.awaiting_read.push_back(thread::current().id());
                drop(state);
                scheduler::yield_thread(); // Wait for rwlock to become available
                // State should already be set by prior thread--no need to change here
            }
            RwLockState::Reading if !rwlock_info.awaiting_write.is_empty() => {
                // Avoid starvation of blocking writers
                rwlock_info.awaiting_read.push_back(thread::current().id());
                drop(state);
                scheduler::yield_thread(); // Wait for write lock to be handled, then read lock
                // State should already be set by prior thread--no need to change here
            }
            RwLockState::Reading => { // The lock is ready to be taken
                rwlock_info.holding_state.insert(thread::current().id());
            }, 
            RwLockState::Available => {
                assert!(rwlock_info.holding_state.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");

                rwlock_info.state = RwLockState::Reading;
                rwlock_info.holding_state.insert(thread::current().id());
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_tryrdlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_tryrdlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        let Some(rwlock_info) = state.rwlocks.get_mut(&rwlock) else {
            crate::abort("`pthread_rwlock_tryrdlock` called on uninitialized rwlock")
        };

        match rwlock_info.state {
            RwLockState::Writing => return libc::EBUSY,
            RwLockState::Reading if !rwlock_info.awaiting_write.is_empty() => return libc::EBUSY,
            RwLockState::Reading => { // The lock is ready to be taken
                rwlock_info.holding_state.insert(thread::current().id());
            }, 
            RwLockState::Available => {
                assert!(rwlock_info.holding_state.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");

                rwlock_info.state = RwLockState::Reading;
                rwlock_info.holding_state.insert(thread::current().id());
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_timedrdlock(
        _lock: *mut libc::pthread_rwlock_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_rwlock_timedrdlock {

        crate::abort("Unimplemented shim `pthread_rwlock_timedrdlock`");
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_clockrdlock(
        _lock: *mut libc::pthread_rwlock_t,
        _clock_id: libc::clockid_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_rwlock_clockrdlock {

        crate::abort("Unimplemented shim `pthread_rwlock_clockrdlock`");
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_wrlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_wrlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        let Some(rwlock_info) = state.rwlocks.get_mut(&rwlock) else {
            crate::abort("`pthread_rwlock_wrlock` called on uninitialized rwlock")
        };

        match rwlock_info.state {
            RwLockState::Available => {
                assert!(rwlock_info.holding_state.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");

                rwlock_info.state = RwLockState::Writing;
                rwlock_info.holding_state.insert(thread::current().id());
            }
            _ => {
                rwlock_info.awaiting_write.push_back(thread::current().id());
                drop(state);
                scheduler::yield_thread(); // Wait for rwlock to become available
                // State should already be set by prior thread--no need to change here
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_trywrlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_trywrlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        let Some(rwlock_info) = state.rwlocks.get_mut(&rwlock) else {
            crate::abort("`pthread_rwlock_trywrlock` called on uninitialized rwlock")
        };

        match rwlock_info.state {
            RwLockState::Available => {
                assert!(rwlock_info.holding_state.is_empty(), "PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");

                rwlock_info.state = RwLockState::Writing;
                rwlock_info.holding_state.insert(thread::current().id());
                0
            }
            _ => libc::EBUSY,
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_timedwrlock(
        _lock: *mut libc::pthread_rwlock_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_rwlock_timedwrlock {

        crate::abort("Unimplemented shim `pthread_rwlock_timedwrlock`");
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_clockwrlock(
        _lock: *mut libc::pthread_rwlock_t,
        _clock_id: libc::clockid_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_rwlock_clockwrlock {

        crate::abort("Unimplemented shim `pthread_rwlock_clockwrlock`");
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_unlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_unlock {

        let mut state = state::fizzle_state().lock().unwrap();
        let rwlock = RwLockId::from(lock);

        let Some(rwlock_info) = state.rwlocks.get_mut(&rwlock) else {
            crate::abort("`pthread_rwlock_unlock` called on uninitialized rwlock")
        };

        if !rwlock_info.holding_state.remove(&thread::current().id()) {
            crate::abort("`pthread_rwlock_unlock` called on rwlock when not locked")
        }

        assert!(rwlock_info.state != RwLockState::Available, "PTRwLock in inconsistent state (RwLockState::Available during unlock procedure)");

        if rwlock_info.holding_state.is_empty() {
            // No more threads holding lock--time to transition to a new state
            match rwlock_info.state {
                RwLockState::Reading => match rwlock_info.awaiting_write.pop_front() {
                    Some(write_thread) => {
                        rwlock_info.holding_state.insert(write_thread);
                        rwlock_info.state = RwLockState::Writing;
                        state.ready_threads.push_back(write_thread);
                    }
                    None => {
                        let threads: Vec<ThreadId> = rwlock_info.awaiting_read.drain(..).collect();
                        rwlock_info.holding_state.extend(threads.clone());
                        if rwlock_info.holding_state.is_empty() { // No threads awaiting reads or writes
                            rwlock_info.state = RwLockState::Available;
                        }
                        
                        for thread in threads {
                            state.ready_threads.push_back(thread);
                        }
                    }
                }
                RwLockState::Writing => if rwlock_info.awaiting_read.is_empty() {
                    if let Some(write_thread) = rwlock_info.awaiting_write.pop_front() {
                        rwlock_info.holding_state.insert(write_thread);
                        state.ready_threads.push_back(write_thread);
                        
                    }else { // No threads waiting reads or writes
                        rwlock_info.state = RwLockState::Available;
                    }
                } else {
                    let threads: Vec<ThreadId> = rwlock_info.awaiting_read.drain(..).collect();
                    rwlock_info.holding_state.extend(threads.clone());
                    rwlock_info.state = RwLockState::Reading;

                    for thread in threads {
                        state.ready_threads.push_back(thread);
                    }
                }
                _ => ()
            }
        }
        
        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_barrier_init(
        lock: *mut libc::pthread_barrier_t,
        _attr: *mut libc::pthread_barrierattr_t,
        count: libc::c_uint
    ) -> libc::c_int => fizzle_pthread_barrier_init {
        // TODO: what about mutexes shared across processes?

        let mut state = state::fizzle_state().lock().unwrap();
        let barrier = BarrierId::from(lock);

        if state.barriers.insert(barrier, BarrierInfo { curr: Vec::new(), needed: count as usize }).is_some() {
            crate::abort("`pthread_barrier_init` called twice on one barrier");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_barrier_destroy(
        lock: *mut libc::pthread_barrier_t
    ) -> libc::c_int => fizzle_pthread_barrier_destroy {

        let mut state = state::fizzle_state().lock().unwrap();
        let barrier = BarrierId::from(lock);

        match state.barriers.remove(&barrier) {
            Some(barrier_info) if barrier_info.curr.len() > 0 => crate::abort("`pthread_barrier_destroy` called on barrier other threads were waiting on"),
            None => crate::abort("`pthread_barrier_destroy` called on uninitialized barrier"),
            _ => ()
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_barrier_wait(
        lock: *mut libc::pthread_barrier_t
    ) -> libc::c_int => fizzle_pthread_barrier_wait {

        let mut state = state::fizzle_state().lock().unwrap();
        let barrier = BarrierId::from(lock);

        let Some(barrier_info) = state.barriers.get_mut(&barrier) else {
            crate::abort("`pthread_barrier_wait` called on uninitialized barrier");
        };

        barrier_info.curr.push(thread::current().id());

        if barrier_info.curr.len() == barrier_info.needed {
            // Release all threads (including this one)
            let threads: Vec<ThreadId> = barrier_info.curr.drain(..).collect();
            for thread_id in threads {
                state.ready_threads.push_back(thread_id);
            }

            -1 // TODO: replace this with `libc::PTHREAD_BARRIER_SERIAL_THREAD` once it exists
        } else {
            scheduler::yield_thread();
            0
        }
    }
}
