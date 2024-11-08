use std::collections::VecDeque;
use std::ptr;
use std::thread::{self, ThreadId};
use std::time::Duration;

use crate::errno::Errno;
use crate::handlers::barrier::{BarrierInfo, BarrierPtr};
use crate::handlers::condvar::CondVarPtr;
use crate::handlers::mutex::*;
use crate::handlers::condvar::*;
use crate::handlers::rwlock::*;
use crate::handlers::spinlock::*;
use crate::handlers::thread::*;
use crate::scheduler::Scheduler;
use crate::{hook_macros, WaitDuration};

// TODO: add these to libc
const PTHREAD_CANCEL_ENABLE: libc::c_int = 0;
const PTHREAD_CANCEL_DISABLE: libc::c_int = 1;
const PTHREAD_CANCEL_DEFERRED: libc::c_int = 0;
const PTHREAD_CANCEL_ASYNCHRONOUS: libc::c_int = 1;
const PTHREAD_MUTEX_FAST_NP: libc::c_int = 0;
const PTHREAD_MUTEX_RECURSIVE_NP: libc::c_int = 1;
const PTHREAD_MUTEX_ERRORCHECK_NP: libc::c_int = 2;

extern "C" {
    pub fn pthread_mutexattr_gettype(
        attr: *const libc::pthread_mutexattr_t,
        kind: *mut libc::c_int,
    ) -> libc::c_int;
}

hook_macros::hook! {
    unsafe fn pthread_create(
        thread: *mut libc::pthread_t,
        attr: *const libc::pthread_attr_t,
        start_routine: PTFunction,
        arg: *mut libc::c_void
    ) -> libc::c_int => fizzle_pthread_create(ctx) {
        crate::strace!("pthread_create(thread={:?}, attr={:?}, start_routine={:?}, arg={:?}) -> ...", thread, attr, start_routine, arg);

        match Scheduler::handle_event(&mut ctx, ThreadCreateEvent::new(thread, attr, start_routine, arg)) {
            Ok(()) => {
                crate::strace!("pthread_create(thread={:?}, attr={:?}, start_routine={:?}, arg={:?}) -> 0", thread, attr, start_routine, arg);
                0
            },
            Err(e) => {
                crate::strace!("pthread_create(thread={:?}, attr={:?}, start_routine={:?}, arg={:?}) -> -1 ({})", thread, attr, start_routine, arg, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_exit(
        retval: *mut libc::c_void
    ) => fizzle_pthread_exit(ctx) {
        crate::strace!("pthread_exit(retval={:?}) -> !", retval);

        let _ = Scheduler::handle_event(&mut ctx, ThreadExitEvent::new(retval));
        unreachable!()
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
    ) -> libc::c_int => fizzle_pthread_join(ctx) {
        crate::strace!("pthread_join(thread={:?}, retval={:?}) -> ...", thread, retval);

        match Scheduler::handle_event(&mut ctx, ThreadJoinEvent::new(thread, retval)) {
            Ok(()) => {
                crate::strace!("pthread_join(thread={:?}, retval={:?}) -> 0", thread, retval);
                0
            },
            Err(e) => {
                crate::strace!("pthread_join(thread={:?}, retval={:?}) -> -1 ({})", thread, retval, e);
                e.set_errno();
                -1
            },
        }
    }
}

// TODO: pthread_cancel
// Save pthread_t and pthread_setcancel_state values
// When a thread tries to cancel another, check cancel state.local. If thread is cancellable, set a
// variable that indicates to the scheduler that it should shut down that thread.
// TODO: deferred cancellation as well--have to hook all known cancellation points and go from there
// TODO: handle cancellation cleanup handlers

hook_macros::hook! {
    unsafe fn pthread_cancel(
        thread: libc::pthread_t
    ) -> libc::c_int => fizzle_pthread_cancel(ctx) {
        crate::strace!("pthread_cancel(thread={:?}) -> ...", thread);

        match Scheduler::handle_event(&mut ctx, ThreadCancelEvent::new(thread)) {
            Ok(()) => {
                crate::strace!("pthread_cancel(thread={:?}) -> 0", thread);
                0
            },
            Err(e) => {
                crate::strace!("pthread_join(thread={:?}) -> -1 ({})", thread, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cleanup_push(
        routine: PTDestructor,
        arg: *mut libc::c_void
    ) => fizzle_pthread_cleanup_push(ctx) {
        crate::strace!("pthread_cleanup_push(routine={:?}, arg={:?}) -> ...", routine, arg);

        match Scheduler::handle_event(&mut ctx, ThreadCleanupPushEvent::new(routine, arg)) {
            Ok(()) => crate::strace!("pthread_cleanup_push(routine={:?}, arg={:?}) -> 0", routine, arg),
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cleanup_pop(
        execute: libc::c_int
    ) => fizzle_pthread_cleanup_pop(ctx) {
        crate::strace!("pthread_cleanup_pop(execute={}) -> ...", execute);

        match Scheduler::handle_event(&mut ctx, ThreadCleanupPopEvent::new(execute != 0)) {
            Ok(Some(routine)) => {
                // TODO: it would be nice to have this within `scheduler.rs`...
                Scheduler::run_outside_hook(&mut ctx, || {
                    routine.call();
                });
            },
            Ok(None) => (),
            Err(()) => unreachable!(),
        }

        crate::strace!("pthread_cleanup_pop(execute={}) -> 0", execute);
    }
}



hook_macros::hook! {
    unsafe fn pthread_key_create(
        key: *mut libc::pthread_key_t,
        destructor: PTDestructor
    ) -> libc::c_int => fizzle_pthread_key_create(ctx) {
        crate::strace!("pthread_key_create(key={:?}, destructor={:?}) -> ...", key, destructor);

        match Scheduler::handle_event(&mut ctx, ThreadKeyCreateEvent::new(key, destructor)) {
            Ok(()) => {
                crate::strace!("pthread_key_create(key={:?}, destructor={:?}) -> 0", key, destructor);
                0
            },
            Err(e) => {
                crate::strace!("pthread_key_create(key={:?}, destructor={:?}) -> -1 ({})", key, destructor, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_key_delete(
        key: libc::pthread_key_t
    ) -> libc::c_int => fizzle_pthread_key_delete(ctx) {
        crate::strace!("pthread_key_delete(key={:?}) -> ...", key);

        match Scheduler::handle_event(&mut ctx, ThreadKeyDeleteEvent::new(key)) {
            Ok(()) => {
                crate::strace!("pthread_key_create(key={:?}) -> 0", key);
                0
            },
            Err(e) => {
                crate::strace!("pthread_key_create(key={:?}) -> -1 ({})", key, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_setspecific(
        key: libc::pthread_key_t,
        pointer: *mut libc::c_void // NOTE: this is actually `*const libc::c_void` in the function definition.
    ) -> libc::c_int => fizzle_pthread_key_setspecific(ctx) {
        let mut state = ctx.acquire();
        state.local.pthread_key_values.get_mut(&key).unwrap().insert(thread::current().id(), pointer);
        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_getspecific(
        key: libc::pthread_key_t
    ) -> *mut libc::c_void => fizzle_pthread_key_getspecific(ctx) {
        let mut state = ctx.acquire();
        *state.local.pthread_key_values.get_mut(&key).unwrap().get_mut(&thread::current().id()).unwrap_or(&mut ptr::null_mut())
    }
}

hook_macros::hook! {
    unsafe fn pthread_tryjoin_np(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void
    ) -> libc::c_int => fizzle_pthread_tryjoin_np(_ctx) {
        crate::report_strict_failure("`pthread_tryjoin_np` unimplemented");
        hook_macros::real!(pthread_tryjoin_np)(thread, retval)
    }
}

hook_macros::hook! {
    unsafe fn pthread_timedjoin_np(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_timedjoin_np(_ctx) {

        crate::report_strict_failure("`pthread_timedjoin_np` unimplemented");
        hook_macros::real!(pthread_timedjoin_np)(thread, retval, abstime)
    }
}

hook_macros::hook! {
    unsafe fn pthread_clockjoin_np(
        thread: libc::pthread_t,
        retval: *mut *mut libc::c_void,
        clock_id: *mut libc::clockid_t,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_clockjoin_np(_ctx) {

        crate::report_strict_failure("`pthread_clockjoin_np` unimplemented");
        hook_macros::real!(pthread_clockjoin_np)(thread, retval, clock_id, abstime)
    }
}

hook_macros::hook! {
    unsafe fn pthread_detach(
        thread: libc::pthread_t
    ) -> libc::c_int => fizzle_pthread_detach(ctx) {
        crate::strace!("pthread_detach(thread={:?}) -> ...", thread);

        match Scheduler::handle_event(&mut ctx, ThreadDetachEvent::new(thread)) {
            Ok(()) => {
                crate::strace!("pthread_detach(thread={:?}) -> 0", thread);
                0
            },
            Err(e) => {
                crate::strace!("pthread_detach(thread={:?}) -> -1 ({})", thread, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_yield(
    ) -> libc::c_int => fizzle_pthread_yield(ctx) {
        crate::strace!("pthread_yield() -> ...");

        match Scheduler::handle_event(&mut ctx, ThreadYieldEvent::new()) {
            Ok(()) => {
                crate::strace!("pthread_yield() -> 0");
                0
            },
            Err(e) => {
                crate::strace!("pthread_yield() -> -1 ({})", e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn sched_yield(
    ) -> libc::c_int => fizzle_sched_yield(ctx) {
        crate::strace!("sched_yield() -> ...");

        match Scheduler::handle_event(&mut ctx, ThreadYieldEvent::new()) {
            Ok(()) => {
                crate::strace!("sched_yield() -> 0");
                0
            },
            Err(e) => {
                crate::strace!("sched_yield() -> -1 ({})", e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_setcancelstate(
        state: libc::c_int,
        old_state: *mut libc::c_int
    ) -> libc::c_int => fizzle_pthread_setcancelstate(ctx) {
        fn cancellable_fmt(cancellable: bool) -> &'static str {
            match cancellable {
                true => "PTHREAD_CANCEL_ENABLE",
                false => "PTHREAD_CANCEL_DISABLE",
            }
        }

        let cancellable = match state {
            PTHREAD_CANCEL_ENABLE => true,
            PTHREAD_CANCEL_DISABLE => false,
            _ => {
                crate::strace!("pthread_setcancelstate(state={}, old_state={:?}) -> -1 (EINVAL)", state, old_state);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        crate::strace!("pthread_setcancelstate(state={}, old_state={:?}) -> ...", cancellable_fmt(cancellable), old_state);

        match Scheduler::handle_event(&mut ctx, ThreadCancellableEvent::new(cancellable)) {
            Ok(old_cancellable) => {
                if !old_state.is_null() {
                    unsafe {
                        *old_state = match old_cancellable {
                            false => PTHREAD_CANCEL_DISABLE,
                            true => PTHREAD_CANCEL_ENABLE,
                        }
                    }

                    crate::strace!("pthread_setcancelstate(state={}, old_state={:?} ({})) -> 0", cancellable_fmt(cancellable), old_state, cancellable_fmt(old_cancellable));
                } else {
                    crate::strace!("pthread_setcancelstate(state={}, old_state=NULL) -> 0", cancellable_fmt(cancellable));
                }

                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_setcanceltype(
        cancel_type: libc::c_int,
        old_type: *mut libc::c_int
    ) -> libc::c_int => fizzle_pthread_setcanceltype(ctx) {
        let cancel_type = match cancel_type {
            PTHREAD_CANCEL_DEFERRED => ThreadCancelType::Deferred,
            PTHREAD_CANCEL_ASYNCHRONOUS => ThreadCancelType::Asynchronous,
            _ => {
                crate::strace!("pthread_setcanceltype(cancel_type={}, old_type={:?}) -> -1 (EINVAL)", cancel_type, old_type);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        crate::strace!("pthread_setcanceltype(cancel_type={}, old_type={:?}) -> ...", cancel_type, old_type);

        match Scheduler::handle_event(&mut ctx, ThreadCancelTypeEvent::new(cancel_type)) {
            Ok(old_cancel_type) => {
                if !old_type.is_null() {
                    unsafe {
                        *old_type = match old_cancel_type {
                            ThreadCancelType::Deferred => PTHREAD_CANCEL_DEFERRED,
                            ThreadCancelType::Asynchronous => PTHREAD_CANCEL_ASYNCHRONOUS,
                        }
                    }

                    crate::strace!("pthread_setcanceltype(cancel_type={}, old_type={:?} ({})) -> 0", cancel_type, old_type, old_cancel_type);
                } else {
                    crate::strace!("pthread_setcanceltype(cancel_type={}, old_type=NULL) -> 0", cancel_type);
                }

                0
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_testcancel(
    ) -> libc::c_int => fizzle_pthread_testcancel(ctx) {
        crate::strace!("pthread_testcancel() -> ...");
        
        match Scheduler::handle_event(&mut ctx, ThreadTestCancelEvent) {
            Ok(()) => {
                crate::strace!("pthread_testcancel() -> 0");
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_init(
        lock: *mut libc::pthread_spinlock_t,
        shared: libc::c_int
    ) -> libc::c_int => fizzle_pthread_spin_init(ctx) {
        fn shared_fmt(shared: bool) -> &'static str {
            match shared {
                true => "PTHREAD_PROCESS_SHARED",
                false => "PTHREAD_PROCESS_PRIVATE",
            }
        }

        let spinlock = SpinlockPtr::from(lock);
        let shared = match shared {
            libc::PTHREAD_PROCESS_PRIVATE => false,
            libc::PTHREAD_PROCESS_SHARED => true,
            _ => {
                crate::strace!("pthread_spin_init(lock={:?}, shared={}) -> -1 (EINVAL)", lock, shared);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        crate::strace!("pthread_spin_init(lock={:?}, shared={}) -> ...", lock, shared_fmt(shared));

        match Scheduler::handle_event(&mut ctx, ThreadSpinInitEvent::new(spinlock, shared)) {
            Ok(()) => {
                crate::strace!("pthread_spin_init(lock={:?}, shared={}) -> 0", spinlock, shared_fmt(shared));
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_destroy(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_destroy(ctx) {
        let spinlock = SpinlockPtr::from(lock);

        crate::strace!("pthread_spin_destroy(lock={:?}) -> ...", spinlock);
        match Scheduler::handle_event(&mut ctx, ThreadSpinDestroyEvent::new(spinlock)) {
            Ok(()) => {
                crate::strace!("pthread_spin_destroy(lock={:?}) -> 0", spinlock);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_lock(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_lock(ctx) {
        let spinlock = SpinlockPtr::from(lock);

        crate::strace!("pthread_spin_lock(lock={:?}) -> ...", spinlock);
        match Scheduler::handle_event(&mut ctx, ThreadSpinLockEvent::new(spinlock, false)) {
            Ok(()) => {
                crate::strace!("pthread_spin_lock(lock={:?}) -> 0", spinlock);
                0
            },
            Err(e) => {
                crate::strace!("pthread_spin_lock(lock={:?}) -> -1 ({})", spinlock, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_trylock(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_trylock(ctx) {
        let spinlock = SpinlockPtr::from(lock);

        crate::strace!("pthread_spin_lock({:?}) -> ...", spinlock);
        match Scheduler::handle_event(&mut ctx, ThreadSpinLockEvent::new(spinlock, true)) {
            Ok(()) => {
                crate::strace!("pthread_spin_lock(lock={:?}) -> 0", spinlock);
                0
            },
            Err(e) => {
                crate::strace!("pthread_spin_lock(lock={:?}) -> -1 ({})", spinlock, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_spin_unlock(
        lock: *mut libc::pthread_spinlock_t
    ) -> libc::c_int => fizzle_pthread_spin_unlock(ctx) {
        let spinlock = SpinlockPtr::from(lock);

        crate::strace!("pthread_spin_unlock(lock={:?}) -> ...", spinlock);
        match Scheduler::handle_event(&mut ctx, ThreadSpinUnlockEvent::new(spinlock)) {
            Ok(()) => {
                crate::strace!("pthread_spin_unlock(lock={:?}) -> 0", spinlock);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_init(
        lock: *mut libc::pthread_mutex_t,
        attr: *const libc::pthread_mutexattr_t
    ) -> libc::c_int => fizzle_pthread_mutex_init(ctx) {
        let mutex = MutexPtr::from(lock);

        let (kind, robustness) = if attr.is_null() {
            crate::strace!("pthread_mutex_init(mutex={:?}, attr=NULL) -> ...", lock);
            (MutexKind::Fast, MutexRobustness::Stalled)

        } else {
            let mut kind: libc::c_int = 0;
            let mut robustness: libc::c_int = 0;
            assert_eq!(pthread_mutexattr_gettype(attr, ptr::addr_of_mut!(kind)), 0);
            assert_eq!(libc::pthread_mutexattr_getrobust(attr, ptr::addr_of_mut!(robustness)), 0);
            let kind = match kind {
                PTHREAD_MUTEX_FAST_NP => MutexKind::Fast,
                PTHREAD_MUTEX_RECURSIVE_NP => MutexKind::Recursive,
                PTHREAD_MUTEX_ERRORCHECK_NP => MutexKind::ErrorChecking,
                _ => {
                    crate::strace!("pthread_mutex_init(mutex={:?}, attr={{type={}, robust={}}}) -> -1 (EINVAL)", lock, kind, robustness);
                    Errno::EINVAL.set_errno();
                    return -1
                }
            };

            let robustness = match robustness {
                libc::PTHREAD_MUTEX_STALLED => MutexRobustness::Stalled,
                libc::PTHREAD_MUTEX_ROBUST => MutexRobustness::Robust,
                _ => {
                    crate::strace!("pthread_mutex_init(mutex={:?}, attr={{type={}, robust={}}}) -> -1 (EINVAL)", lock, kind, robustness);
                    Errno::EINVAL.set_errno();
                    return -1
                }
            };

            crate::strace!("pthread_mutex_init(mutex={:?}, attr={{type={}, robust={}}}) -> ...", lock, kind, robustness);

            (kind, robustness)
        };

        match Scheduler::handle_event(&mut ctx, MutexInitEvent::new(mutex, kind, robustness)) {
            Ok(()) => {
                crate::strace!("pthread_mutex_init(mutex={:?}, ...) -> 0", mutex);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_destroy(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_destroy(ctx) {
        let mutex = MutexPtr::from(lock);

        crate::strace!("pthread_mutex_destroy(mutex={:?}) -> ...", lock);

        match Scheduler::handle_event(&mut ctx, MutexDestroyEvent::new(mutex)) {
            Ok(()) => {
                crate::strace!("pthread_mutex_destroy(mutex={:?}) -> 0", mutex);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_destroy(mutex={:?}) -> -1 ({})", mutex, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_lock(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_lock(ctx) {
        let mutex = MutexPtr::from(lock);

        crate::strace!("pthread_mutex_lock(mutex={:?}) -> ...", lock);

        match Scheduler::handle_event(&mut ctx, MutexLockEvent::new(mutex, WaitDuration::Indefinite)) {
            Ok(()) => {
                crate::strace!("pthread_mutex_lock(mutex={:?}) -> 0", mutex);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_lock(mutex={:?}) -> -1 ({})", mutex, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_trylock(
        lock: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_trylock(ctx) {
        let mutex = MutexPtr::from(lock);

        crate::strace!("pthread_mutex_trylock(mutex={:?}) -> ...", lock);

        match Scheduler::handle_event(&mut ctx, MutexLockEvent::new(mutex, WaitDuration::Immediate)) {
            Ok(()) => {
                crate::strace!("pthread_mutex_trylock(mutex={:?}) -> 0", mutex);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_trylock(mutex={:?}) -> -1 ({})", mutex, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_timedlock(
        lock: *mut libc::pthread_mutex_t,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_mutex_timedlock(ctx) {
        let mutex = MutexPtr::from(lock);

        if abstime.is_null() || unsafe { (*abstime).tv_sec < 0 || (*abstime).tv_nsec < 0 } {
            crate::strace!("pthread_mutex_timedlock(mutex={:?}, abstime={:?}) -> -1 (EINVAL)", lock, abstime);
            Errno::EINVAL.set_errno();
            return -1
        }

        let duration = Duration::from_secs(unsafe { (*abstime).tv_sec as u64 }) + Duration::from_nanos(unsafe { (*abstime).tv_nsec as u64 });

        crate::strace!("pthread_mutex_timedlock(mutex={:?}, abstime={:?}) -> ...", lock, duration);

        match Scheduler::handle_event(&mut ctx, MutexLockEvent::new(mutex, WaitDuration::Timed(duration))) {
            Ok(()) => {
                crate::strace!("pthread_mutex_timedlock(mutex={:?}, abstime={:?}) -> 0", mutex, duration);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_timedlock(mutex={:?}, abstime={:?}) -> -1 ({})", mutex, duration, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_clocklock(
        lock: *mut libc::pthread_mutex_t,
        clock_id: libc::clockid_t,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_mutex_clocklock(ctx) {
        let mutex = MutexPtr::from(lock);

        if abstime.is_null() || unsafe { (*abstime).tv_sec < 0 || (*abstime).tv_nsec < 0 } {
            crate::strace!("pthread_mutex_timedlock(mutex={:?}, abstime={:?}) -> -1 (EINVAL)", lock, abstime);
            Errno::EINVAL.set_errno();
            return -1
        }

        let duration = Duration::from_secs(unsafe { (*abstime).tv_sec as u64 }) + Duration::from_nanos(unsafe { (*abstime).tv_nsec as u64 });

        crate::strace!("pthread_mutex_clocklock(mutex={:?}, clock_id={}, abstime={:?}) -> ...", lock, clock_id, duration);

        match Scheduler::handle_event(&mut ctx, MutexLockEvent::new(mutex, WaitDuration::Timed(duration))) {
            Ok(()) => {
                crate::strace!("pthread_mutex_timedlock(mutex={:?}, clock_id={}, abstime={:?}) -> 0", mutex, clock_id, duration);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_timedlock(mutex={:?}, clock_id={}, abstime={:?}) -> -1 ({})", mutex, clock_id, duration, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_unlock(
        mutex: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_unlock(ctx) {
        let lock = MutexPtr::from(mutex);

        crate::strace!("pthread_mutex_unlock(mutex={:?}) -> ...", mutex);

        match Scheduler::handle_event(&mut ctx, MutexUnlockEvent::new(lock)) {
            Ok(()) => {
                crate::strace!("pthread_mutex_unlock(mutex={:?}) -> 0", mutex);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_timedlock(mutex={:?}) -> -1 ({})", mutex, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_mutex_consistent(
        mutex: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_mutex_consistent(ctx) {
        let lock = MutexPtr::from(mutex);

        crate::strace!("pthread_mutex_consistent(mutex={:?}) -> ...", mutex);

        match Scheduler::handle_event(&mut ctx, MutexConsistentEvent::new(lock)) {
            Ok(()) => {
                crate::strace!("pthread_mutex_consistent(mutex={:?}) -> 0", mutex);
                0
            },
            Err(e) => {
                crate::strace!("pthread_mutex_consistent(mutex={:?}) -> -1 ({})", mutex, e);
                e.set_errno();
                -1
            }
        }
    }
}

// TODO: pthread_cond_clockwait

hook_macros::hook! {
    unsafe fn pthread_cond_init(
        lock: *mut libc::pthread_cond_t,
        attr: *mut libc::pthread_condattr_t
    ) -> libc::c_int => fizzle_pthread_cond_init(ctx) {
        let cond = CondVarPtr::from(lock);

        crate::strace!("pthread_cond_init(cond={:?}, attr={:?}) -> ...", lock, attr);

        match Scheduler::handle_event(&mut ctx, CondInitEvent::new(cond)) {
            Ok(()) => {
                crate::strace!("pthread_cond_init(cond={:?}, attr={:?}) -> 0", lock, attr);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_destroy(
        lock: *mut libc::pthread_cond_t
    ) -> libc::c_int => fizzle_pthread_cond_destroy(ctx) {
        let cond = CondVarPtr::from(lock);

        crate::strace!("pthread_cond_destroy(cond={:?}) -> ...", lock);

        match Scheduler::handle_event(&mut ctx, CondDestroyEvent::new(cond)) {
            Ok(()) => {
                crate::strace!("pthread_cond_destroy(cond={:?}) -> 0", lock);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_signal(
        lock: *mut libc::pthread_cond_t
    ) -> libc::c_int => fizzle_pthread_cond_signal(ctx) {
        let cond = CondVarPtr::from(lock);

        crate::strace!("pthread_cond_signal(cond={:?}) -> ...", lock);

        match Scheduler::handle_event(&mut ctx, CondSignalEvent::new(cond)) {
            Ok(()) => {
                crate::strace!("pthread_cond_signal(cond={:?}) -> 0", lock);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_broadcast(
        lock: *mut libc::pthread_cond_t
    ) -> libc::c_int => fizzle_pthread_cond_broadcast(ctx) {
        let cond = CondVarPtr::from(lock);

        crate::strace!("pthread_cond_broadcast(cond={:?}) -> ...", lock);

        match Scheduler::handle_event(&mut ctx, CondBroadcastEvent::new(cond)) {
            Ok(()) => {
                crate::strace!("pthread_cond_broadcast(cond={:?}) -> 0", lock);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_wait(
        cond: *mut libc::pthread_cond_t,
        mutex: *mut libc::pthread_mutex_t
    ) -> libc::c_int => fizzle_pthread_cond_wait(ctx) {
        let cond_id = CondVarPtr::from(cond);
        let mutex_id = MutexPtr::from(mutex);

        crate::strace!("pthread_cond_wait(cond={:?}, mutex={:?}) -> ...", cond, mutex);

        match Scheduler::handle_event(&mut ctx, CondWaitEvent::new(cond_id, mutex_id, WaitDuration::Indefinite)) {
            Ok(()) => {
                crate::strace!("pthread_cond_wait(cond={:?}, mutex={:?}) -> 0", cond, mutex);
                0
            },
            Err(e) => {
                crate::strace!("pthread_cond_wait(cond={:?}, mutex={:?}) -> -1 ({})", cond, mutex, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_timedwait(
        cond: *mut libc::pthread_cond_t,
        mutex: *mut libc::pthread_mutex_t,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_cond_timedwait(ctx) {
        let cond_id = CondVarPtr::from(cond);
        let mutex_id = MutexPtr::from(mutex);

        if abstime.is_null() || unsafe { (*abstime).tv_sec < 0 || (*abstime).tv_nsec < 0 } {
            crate::strace!("pthread_cond_timedwait(cond={:?}, mutex={:?}, abstime={:?}) -> -1 (EINVAL)", cond, mutex, abstime);
            Errno::EINVAL.set_errno();
            return -1
        }

        let duration = Duration::from_secs(unsafe { (*abstime).tv_sec as u64 }) + Duration::from_nanos(unsafe { (*abstime).tv_nsec as u64 });

        crate::strace!("pthread_cond_timedwait(cond={:?}, mutex={:?}, abstime={:?}) -> ...", cond, mutex, duration);

        match Scheduler::handle_event(&mut ctx, CondWaitEvent::new(cond_id, mutex_id, WaitDuration::Timed(duration))) {
            Ok(()) => {
                crate::strace!("pthread_cond_timedwait(cond={:?}, mutex={:?}, abstime={:?}) -> 0", cond, mutex, duration);
                0
            },
            Err(e) => {
                crate::strace!("pthread_cond_timedwait(cond={:?}, mutex={:?}, abstime={:?}) -> -1 ({})", cond, mutex, duration, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_cond_clockwait(
        cond: *mut libc::pthread_cond_t,
        mutex: *mut libc::pthread_mutex_t,
        clock_id: libc::clockid_t,
        abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_cond_clockwait(ctx) {
        let cond_id = CondVarPtr::from(cond);
        let mutex_id = MutexPtr::from(mutex);

        if abstime.is_null() || unsafe { (*abstime).tv_sec < 0 || (*abstime).tv_nsec < 0 } {
            crate::strace!("pthread_cond_clockwait(cond={:?}, mutex={:?}, clock_id={}, abstime={:?}) -> -1 (EINVAL)", cond, mutex, clock_id, abstime);
            Errno::EINVAL.set_errno();
            return -1
        }

        let duration = Duration::from_secs(unsafe { (*abstime).tv_sec as u64 }) + Duration::from_nanos(unsafe { (*abstime).tv_nsec as u64 });

        crate::strace!("pthread_cond_clockwait(cond={:?}, mutex={:?}, clock_id={}, abstime={:?}) -> ...", cond, mutex, clock_id, duration);

        match Scheduler::handle_event(&mut ctx, CondWaitEvent::new(cond_id, mutex_id, WaitDuration::Timed(duration))) {
            Ok(()) => {
                crate::strace!("pthread_cond_clockwait(cond={:?}, mutex={:?}, clock_id={}, abstime={:?}) -> 0", cond, mutex, clock_id, duration);
                0
            },
            Err(e) => {
                crate::strace!("pthread_cond_clockwait(cond={:?}, mutex={:?}, clock_id={}, abstime={:?}) -> -1 ({})", cond, mutex, clock_id, duration, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_init(
        lock: *mut libc::pthread_rwlock_t,
        _attr: *mut libc::pthread_rwlockattr_t
    ) -> libc::c_int => fizzle_pthread_rwlock_init(ctx) {
        let mut state = ctx.acquire();
        // TODO: what about mutexes shared across processes?

        let rwlock = RwLockPtr::from(lock);

        if state.local.rwlocks.insert(rwlock, RwLockInfo::default()).is_some() {
            panic!("[UB] `pthread_rwlock_init` called twice on one rwlock");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_destroy(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_destroy(ctx) {
        let mut state = ctx.acquire();

        let rwlock = RwLockPtr::from(lock);

        match state.local.rwlocks.remove(&rwlock) {
            Some(rwlock_info) => {
                if rwlock_info.state != RwLockState::Available {
                    panic!("[UB] `pthread_rwlock_destroy` called on locked rwlock") // Undefined behavior
                }

                if !rwlock_info.awaiting_read.is_empty() || !rwlock_info.awaiting_read.is_empty() || !rwlock_info.awaiting_read.is_empty() {
                    panic!("inconsistent fizzle RwLock state in `pthread_rwlock_destroy`");
                }
            },
            None => {
                let res = libc::pthread_rwlock_trywrlock(lock);
                if res < 0 {
                    panic!("[UB] `pthread_rwlock_destroy` called on uninitialized rwlock")
                }
            }
        };

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_rdlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_rdlock(ctx) {
        let mut state = ctx.acquire();

        let rwlock = RwLockPtr::from(lock);

        let rwlock_info = match state.local.rwlocks.get_mut(&rwlock) {
            Some(rwlock_info) => rwlock_info,
            None => {
                let res = libc::pthread_rwlock_trywrlock(lock);
                if res < 0 {
                    panic!("[UB] `pthread_rwlock_rdlock` called on uninitialized rwlock")
                } else {
                    // This was a statically-initialized mutex--add it to our queue (and leave locked)
                    state.local.rwlocks.insert(rwlock, RwLockInfo::default());
                    state.local.rwlocks.get_mut(&rwlock).unwrap()
                }
            }
        };

        match rwlock_info.state {
            RwLockState::Writing => {
                rwlock_info.awaiting_read.push_back(thread::current().id());
                drop(state);
                ctx.yield_thread();
                // State should already be set by prior thread--no need to change here
            }
            RwLockState::Reading if !rwlock_info.awaiting_write.is_empty() => {
                // Avoid starvation of blocking writers
                rwlock_info.awaiting_read.push_back(thread::current().id());
                // Wait for write lock to be handled, then read lock
                drop(state);
                ctx.yield_thread();
                // State should already be set by prior thread--no need to change here
            }
            RwLockState::Reading => { // The lock is ready to be taken
                rwlock_info.holding_state.insert(thread::current().id());
            },
            RwLockState::Available => {
                if !rwlock_info.holding_state.is_empty() {
                    panic!("fizzle RwLock in inconsistent state (RwLockState::Available when some threads still holding state)");
                }

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
    ) -> libc::c_int => fizzle_pthread_rwlock_tryrdlock(ctx) {
        let mut state = ctx.acquire();

        let rwlock = RwLockPtr::from(lock);

        let rwlock_info = match state.local.rwlocks.get_mut(&rwlock) {
            Some(rwlock_info) => rwlock_info,
            None => {
                let res = libc::pthread_rwlock_trywrlock(lock);
                if res < 0 {
                    panic!("[UB] `pthread_rwlock_tryrdlock` called on uninitialized rwlock")
                } else {
                    // This was a statically-initialized mutex--add it to our queue (and leave locked)
                    state.local.rwlocks.insert(rwlock, RwLockInfo::default());
                    state.local.rwlocks.get_mut(&rwlock).unwrap()
                }
            }
        };

        match rwlock_info.state {
            RwLockState::Writing => return libc::EBUSY,
            RwLockState::Reading if !rwlock_info.awaiting_write.is_empty() => return libc::EBUSY,
            RwLockState::Reading => { // The lock is ready to be taken
                rwlock_info.holding_state.insert(thread::current().id());
            },
            RwLockState::Available => {
                if !rwlock_info.holding_state.is_empty() {
                    panic!("fizzle RwLock in inconsistent state (RwLockState::Available when some threads still holding state)");
                }

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
    ) -> libc::c_int => fizzle_pthread_rwlock_timedrdlock(_ctx) {

        crate::report_strict_failure("`pthread_rwlock_timedrdlock` unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_clockrdlock(
        _lock: *mut libc::pthread_rwlock_t,
        _clock_id: libc::clockid_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_rwlock_clockrdlock(_ctx) {

        crate::report_strict_failure("`pthread_rwlock_clockrdlock` unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_wrlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_wrlock(ctx) {
        let mut state = ctx.acquire();

        let rwlock = RwLockPtr::from(lock);

        let rwlock_info = match state.local.rwlocks.get_mut(&rwlock) {
            Some(rwlock_info) => rwlock_info,
            None => {
                let res = libc::pthread_rwlock_trywrlock(lock);
                if res < 0 {
                    panic!("[UB] `pthread_rwlock_wrlock` called on uninitialized rwlock")
                } else {
                    // This was a statically-initialized mutex--add it to our queue (and leave locked)
                    state.local.rwlocks.insert(rwlock, RwLockInfo::default());
                    state.local.rwlocks.get_mut(&rwlock).unwrap()
                }
            }
        };

        match rwlock_info.state {
            RwLockState::Available => {
                if !rwlock_info.holding_state.is_empty() {
                    panic!("PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");
                }

                rwlock_info.state = RwLockState::Writing;
                rwlock_info.holding_state.insert(thread::current().id());
            }
            _ => {
                rwlock_info.awaiting_write.push_back(thread::current().id());
                // Wait for rwlock to become available
                drop(state);
                ctx.yield_thread();
                // State should already be set by prior thread--no need to change here
            }
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_trywrlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_trywrlock(ctx) {
        let mut state = ctx.acquire();

        let rwlock = RwLockPtr::from(lock);

        let rwlock_info = match state.local.rwlocks.get_mut(&rwlock) {
            Some(rwlock_info) => rwlock_info,
            None => {
                let res = libc::pthread_rwlock_trywrlock(lock);
                if res < 0 {
                    panic!("[UB] `pthread_rwlock_trywrlock` called on uninitialized rwlock")
                } else {
                    // This was a statically-initialized mutex--add it to our queue (and leave locked)
                    state.local.rwlocks.insert(rwlock, RwLockInfo::default());
                    state.local.rwlocks.get_mut(&rwlock).unwrap()
                }
            }
        };

        match rwlock_info.state {
            RwLockState::Available => {
                if !rwlock_info.holding_state.is_empty() {
                    panic!("PTRwLock in inconsistent state (RwLockState::Available when some threads still holding state)");
                }

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
    ) -> libc::c_int => fizzle_pthread_rwlock_timedwrlock(_ctx) {

        crate::report_strict_failure("`pthread_rwlock_timedwrlock` unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_clockwrlock(
        _lock: *mut libc::pthread_rwlock_t,
        _clock_id: libc::clockid_t,
        _abstime: *const libc::timespec
    ) -> libc::c_int => fizzle_pthread_rwlock_clockwrlock(_ctx) {

        crate::report_strict_failure("`pthread_rwlock_clockwrlock` unimplemented");
        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_rwlock_unlock(
        lock: *mut libc::pthread_rwlock_t
    ) -> libc::c_int => fizzle_pthread_rwlock_unlock(ctx) {
        let mut state = ctx.acquire();

        let rwlock = RwLockPtr::from(lock);

        let Some(rwlock_info) = state.local.rwlocks.get_mut(&rwlock) else {
            panic!("[UB] `pthread_rwlock_unlock` called on uninitialized rwlock")
        };

        if !rwlock_info.holding_state.remove(&thread::current().id()) {
            panic!("[UB] `pthread_rwlock_unlock` called on rwlock when not locked")
        }

        if rwlock_info.state == RwLockState::Available {
            panic!("fizzle RwLock in inconsistent state (RwLockState::Available during unlock procedure)");
        }

        if rwlock_info.holding_state.is_empty() {
            // No more threads holding lock--time to transition to a new state
            match rwlock_info.state {
                RwLockState::Reading => match rwlock_info.awaiting_write.pop_front() {
                    Some(write_thread) => {
                        rwlock_info.holding_state.insert(write_thread);
                        rwlock_info.state = RwLockState::Writing;
                        state.mark_thread_ready(write_thread);
                    }
                    None => {
                        let threads: Vec<ThreadId> = rwlock_info.awaiting_read.drain(..).collect();
                        rwlock_info.holding_state.extend(threads.clone());
                        if rwlock_info.holding_state.is_empty() { // No threads awaiting reads or writes
                            rwlock_info.state = RwLockState::Available;
                        }

                        for thread in threads {
                            state.mark_thread_ready(thread);
                        }
                    }
                }
                RwLockState::Writing => if rwlock_info.awaiting_read.is_empty() {
                    if let Some(write_thread) = rwlock_info.awaiting_write.pop_front() {
                        rwlock_info.holding_state.insert(write_thread);
                        state.mark_thread_ready(write_thread);

                    }else { // No threads waiting reads or writes
                        rwlock_info.state = RwLockState::Available;
                    }
                } else {
                    let threads: Vec<ThreadId> = rwlock_info.awaiting_read.drain(..).collect();
                    rwlock_info.holding_state.extend(threads.clone());
                    rwlock_info.state = RwLockState::Reading;

                    for thread in threads {
                        state.mark_thread_ready(thread);
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
    ) -> libc::c_int => fizzle_pthread_barrier_init(ctx) {
        let mut state = ctx.acquire();
        // TODO: what about mutexes shared across processes?

        let barrier = BarrierPtr::from(lock);

        if state.local.barriers.insert(barrier, BarrierInfo { curr: Vec::new(), needed: count as usize }).is_some() {
            panic!("[UB] `pthread_barrier_init` called twice on one barrier");
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_barrier_destroy(
        lock: *mut libc::pthread_barrier_t
    ) -> libc::c_int => fizzle_pthread_barrier_destroy(ctx) {
        let mut state = ctx.acquire();

        let barrier = BarrierPtr::from(lock);

        match state.local.barriers.remove(&barrier) {
            Some(barrier_info) if !barrier_info.curr.is_empty() => panic!("[UB] `pthread_barrier_destroy` called on barrier other threads were waiting on"),
            None => panic!("[UB] `pthread_barrier_destroy` called on uninitialized barrier"),
            _ => ()
        }

        0
    }
}

hook_macros::hook! {
    unsafe fn pthread_barrier_wait(
        lock: *mut libc::pthread_barrier_t
    ) -> libc::c_int => fizzle_pthread_barrier_wait(ctx) {
        let mut state = ctx.acquire();

        let barrier = BarrierPtr::from(lock);

        let Some(barrier_info) = state.local.barriers.get_mut(&barrier) else {
            panic!("[UB] `pthread_barrier_wait` called on uninitialized barrier");
        };

        barrier_info.curr.push(thread::current().id());

        if barrier_info.curr.len() == barrier_info.needed {
            // Release all threads (including this one)
            let threads: Vec<ThreadId> = barrier_info.curr.drain(..).collect();
            for thread_id in threads {
                state.mark_thread_ready(thread_id);
            }

            -1 // TODO: replace this with `libc::PTHREAD_BARRIER_SERIAL_THREAD` once it exists
        } else {
            drop(state);
            ctx.yield_thread();
            0
        }
    }
}

hook_macros::hook! {
    unsafe fn setns(
        _fd: libc::c_int,
        _nstype: libc::c_int
    ) => fizzle_setns(_ctx) {
        unimplemented!("setns()")
    }
}
