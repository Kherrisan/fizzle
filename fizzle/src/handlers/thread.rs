use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::thread::ThreadId;
use std::time::Duration;
use std::{ptr, thread};

use fxhash::FxBuildHasher;

use crate::errno::Errno;
use crate::scheduler::{
    fizzle_alloc, fizzle_singleton, Event, FizzleSingleton, Outcome, Scheduler, TaskResult,
    TerminationMethod, YieldUntil,
};
use crate::semaphore::Semaphore;
use crate::state::{set_entered_handler, FizzleState};

use super::id::Worker;
use super::mutex::MutexPtr;
use super::signal::SignalSet;

extern "C" {
    /*
    pub fn pthread_attr_setsigmask_np(
        attr: *mut libc::pthread_attr_t,
        sigmask: *const libc::sigset_t,
    ) -> libc::c_int;

    pub fn pthread_attr_getsigmask_np(
        attr: *const libc::pthread_attr_t,
        sigmask: *mut libc::sigset_t,
    ) -> libc::c_int;
    */

    pub fn pthread_attr_getdetachstate(
        attr: *const libc::pthread_attr_t,
        detachstate: *mut libc::c_int,
    ) -> libc::c_int;
}

pub type PtFunction = unsafe extern "C" fn(*mut libc::c_void) -> *mut libc::c_void;
pub type PTDestructor = unsafe extern "C" fn(*mut libc::c_void);

#[derive(Debug, Clone, Copy)]
pub enum ThreadCancelType {
    Deferred,
    Asynchronous,
}

impl Display for ThreadCancelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deferred => f.write_str("PTHREAD_CANCEL_DEFERRED"),
            Self::Asynchronous => f.write_str("PTHREAD_CANCEL_ASYNCHRONOUS"),
        }
    }
}

/// The ID associated with a given thread.
///
/// Equivalent to a tid_t.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tid(usize);

impl Tid {
    pub fn from_raw(tid: libc::pid_t) -> Self {
        Self(tid.try_into().unwrap())
    }

    pub fn as_raw(&self) -> libc::pid_t {
        self.0 as i32
    }
}

#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub id: ThreadId,
    pub detached: bool,
    pub cancellable: bool,
    pub cancel_type: ThreadCancelType,
    pub cancel_requested: bool,
    pub held_mutexes: HashSet<MutexPtr, FxBuildHasher>,
}

impl ThreadInfo {
    pub fn new(id: ThreadId, detached: bool, cancellable: bool) -> Self {
        Self {
            id,
            detached,
            cancellable,
            cancel_type: ThreadCancelType::Deferred,
            cancel_requested: false,
            held_mutexes: Default::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadTermination {
    Cancellation,
    Exit(*mut libc::c_void),
    SigTerm,
}

#[derive(Clone, Copy, Debug)]
pub struct PThreadRoutine {
    pub function: PTDestructor,
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

#[derive(Clone)]
#[repr(C)]
pub struct PtCreateWrapper {
    wrapped_fn: PtFunction,
    wrapped_arg: *mut libc::c_void,
    sigmask: Option<SignalSet>,
    detached: bool,
}

pub enum ThreadCreateState {
    Start,
    Finish,
}

extern "C" fn pt_wrapper_fn(arg: *mut libc::c_void) -> *mut libc::c_void {
    // Before we do ANYTHING, we need to set this to avoid accidental preload hook recursion
    set_entered_handler(true);
    // SAFETY: only one ctx at the time (so that it in turn enforces only one `state` alias at a time...)
    let mut ctx = unsafe { fizzle_singleton() };

    let wrapped_arg = unsafe { (arg.cast::<PtCreateWrapper>()).as_mut().unwrap() };

    // SAFETY: the FizzleState can be acquired here because we know startup initialization has
    // already run for this process (otherwise how could this pt_wrapper_fn be called?).
    let mut state = ctx.acquire();
    let tid = state.global.next_tid();
    let worker_id = state.current_worker();

    state
        .global
        .worker_locks
        .insert(worker_id, Semaphore::new_rc_in(0, true, fizzle_alloc()));
    state.local.initialize_thread(tid, wrapped_arg.sigmask);
    drop(state);

    let create_fn = wrapped_arg.wrapped_fn;
    let create_arg = wrapped_arg.wrapped_arg;

    let res = Scheduler::run_outside_hook(&mut ctx, || unsafe { (create_fn)(create_arg) });

    // Thread has exited...
    let _ = Scheduler::handle_event(&mut ctx, ThreadExitEvent::new(res));
    unreachable!()
}

#[derive(Clone)]
pub struct ThreadCreateContext {
    pthread: *mut libc::pthread_t,
    attrs: *const libc::pthread_attr_t,
    arg: *mut libc::c_void,
}

unsafe impl Send for ThreadCreateContext {}

pub struct ThreadCreateEvent {
    pthread: *mut libc::pthread_t,
    attrs: *const libc::pthread_attr_t,
    wrapper: PtCreateWrapper,
    state: ThreadCreateState,
}

impl ThreadCreateEvent {
    pub fn new(
        pthread: *mut libc::pthread_t,
        attrs: *const libc::pthread_attr_t,
        f: PtFunction,
        arg: *mut libc::c_void,
    ) -> Self {
        // If the attributes contain a sigmask, note it (but remove the actual sigmask)
        /*
        let sigmask = match attrs.is_null() {
            true => None,
            false => {
                // TODO:
                let sigmask = Self::get_attr_sigmask(attrs);
                Self::clear_attr_sigmask(attrs.cast::<libc::pthread_attr_t>()); // TODO: BUG: undefined behavior--fix
                Some(SignalSet::from_sigset(sigmask))
            }
        };
        */
        // TODO: track pthread_attr_t instances instead
        let sigmask = None;

        let detached = if attrs.is_null() {
            false
        } else {
            let mut detach_state: libc::c_int = 0;
            assert_eq!(
                unsafe { pthread_attr_getdetachstate(attrs, ptr::addr_of_mut!(detach_state)) },
                0
            );
            detach_state != 0
        };

        let wrapper = PtCreateWrapper {
            wrapped_fn: f,
            wrapped_arg: arg,
            sigmask,
            detached,
        };

        Self {
            pthread,
            attrs,
            wrapper,
            state: ThreadCreateState::Start,
        }
    }
}

impl Event for ThreadCreateEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            ThreadCreateState::Start => {
                self.state = ThreadCreateState::Finish;

                // Initialize AFL (forkservers and multithreading don't play well)
                #[cfg(feature = "afl")]
                if !state.global.afl_shmem_initialized {
                    state.global.afl_shmem_initialized = true;

                    #[cfg(feature = "pcr")]
                    unsafe {
                        crate::__afl_sharedmem_fuzzing = 1;
                    }

                    log::debug!("calling __afl_manual_init()");
                    unsafe {
                        crate::__afl_manual_init();
                    }
                    log::debug!("__afl_manual_init finished");
                }

                // SAFETY: `self.wrapper` is guaranteed to remain in scope untio `pt_wrapper_fn`
                // is called; it copies the internal pointers in `self.wrapper`.
                let thread_ctx = ThreadCreateContext {
                    pthread: self.pthread,
                    attrs: self.attrs,
                    arg: (&raw mut self.wrapper).cast(),
                };

                Outcome::RunTask(
                    Box::new_in(
                        move |_| {
                            thread_create(thread_ctx);
                            TaskResult::Suspend
                        },
                        fizzle_alloc(),
                    ),
                    YieldUntil::Reschedule(Duration::ZERO),
                )
            }
            ThreadCreateState::Finish => Outcome::Success(()),
        }
    }
}

fn thread_create(thread_ctx: ThreadCreateContext) {
    let res = unsafe {
        libc::pthread_create(
            thread_ctx.pthread,
            thread_ctx.attrs,
            pt_wrapper_fn,
            thread_ctx.arg,
        )
    };
    assert_eq!(res, 0);
}

pub struct ThreadExitRetval {
    retval: *mut libc::c_void,
}

impl ThreadExitRetval {
    pub fn new(retval: *mut libc::c_void) -> Self {
        Self { retval }
    }
}

unsafe impl Send for ThreadExitRetval {}

pub struct ThreadExitEvent {
    retval: *mut libc::c_void,
}

impl ThreadExitEvent {
    pub fn new(retval: *mut libc::c_void) -> Self {
        Self { retval }
    }
}

impl Event for ThreadExitEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let retval = ThreadExitRetval::new(self.retval);

        Outcome::RunTask(
            Box::new_in(move |ctx| exit_thread(ctx, retval), fizzle_alloc()),
            YieldUntil::None,
        )
    }
}

fn exit_thread(ctx: &mut FizzleSingleton, retval: ThreadExitRetval) -> ! {
    Scheduler::terminate_thread(ctx, TerminationMethod::ThreadExit(retval.retval))
}

pub enum ThreadJoinState {
    Start,
    Finish,
}

pub struct ThreadJoinEvent {
    thread: libc::pthread_t,
    retval: *mut *mut libc::c_void,
    state: ThreadJoinState,
}

impl ThreadJoinEvent {
    pub fn new(thread: libc::pthread_t, retval: *mut *mut libc::c_void) -> Self {
        Self {
            thread,
            retval,
            state: ThreadJoinState::Start,
        }
    }
}

impl Event for ThreadJoinEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            ThreadJoinState::Start => {
                self.state = ThreadJoinState::Finish;

                let Some(target_thread) = state.local.pthreads.remove(&self.thread) else {
                    return Outcome::Error(Errno::EDEADLK);
                };

                if !state.local.terminated_threads.contains(&target_thread.id) {
                    // Target thread has not yet terminated--add it to list of threads awaiting death of target
                    match state.local.awaiting_thread_death.entry(target_thread.id) {
                        Entry::Occupied(mut o) => o.get_mut().push(thread::current().id()),
                        Entry::Vacant(v) => {
                            v.insert(vec![thread::current().id()]);
                        }
                    }

                    Outcome::Yield(YieldUntil::None)
                } else {
                    state.local.terminated_threads.remove(&target_thread.id);
                    Outcome::Yield(YieldUntil::Immediate)
                }
            }
            ThreadJoinState::Finish => {
                // Waiting thread has now terminated--join properly
                let ret = unsafe { libc::pthread_join(self.thread, self.retval) };
                match ret {
                    0.. => Outcome::Success(()),
                    ..=-1 => Outcome::Error(Errno::get_errno()),
                }
            }
        }
    }
}

pub struct ThreadDetachEvent {
    thread: libc::pthread_t,
}

impl ThreadDetachEvent {
    pub fn new(thread: libc::pthread_t) -> Self {
        Self { thread }
    }
}

impl Event for ThreadDetachEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match state.local.pthreads.get_mut(&self.thread) {
            None => Outcome::Error(Errno::ESRCH),
            Some(thread_info) => {
                if thread_info.detached {
                    panic!("[UB] detached thread that was already detached");
                }
                thread_info.detached = true;
                Outcome::Success(())
            }
        }
    }
}

pub enum ThreadCancelState {
    Start,
    Finish,
}

pub struct ThreadCancelEvent {
    thread: libc::pthread_t,
    state: ThreadCancelState,
}

impl ThreadCancelEvent {
    pub fn new(thread: libc::pthread_t) -> Self {
        Self {
            thread,
            state: ThreadCancelState::Start,
        }
    }
}

impl Event for ThreadCancelEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            ThreadCancelState::Start => {
                self.state = ThreadCancelState::Finish;

                let Some(target_thread) = state.local.pthreads.get_mut(&self.thread) else {
                    return Outcome::Error(Errno::ESRCH);
                };

                if !target_thread.cancellable {
                    target_thread.cancel_requested = true;
                    return Outcome::Success(());
                }

                if self.thread == unsafe { libc::pthread_self() } {
                    return Outcome::RunTask(
                        Box::new_in(
                            move |ctx| {
                                Scheduler::terminate_thread(ctx, TerminationMethod::Cancellation)
                            },
                            fizzle_alloc(),
                        ),
                        YieldUntil::None,
                    );
                }

                let thread_id = target_thread.id;

                Outcome::RunTask(
                    Box::new_in(
                        move |ctx| {
                            if thread::current().id() != thread_id {
                                let mut state = ctx.acquire();
                                let pid = state.local.process_info.borrow().pid;
                                state.mark_thread_ready(thread::current().id());
                                let target_worker = Worker { pid, thread_id };
                                let target_sem = state
                                    .global
                                    .worker_locks
                                    .get(&target_worker)
                                    .unwrap()
                                    .clone();
                                state.global.tasks.push_front(Box::new_in(
                                    |ctx| {
                                        Scheduler::terminate_thread(
                                            ctx,
                                            TerminationMethod::Cancellation,
                                        )
                                    },
                                    fizzle_alloc(),
                                ));
                                drop(state);

                                log::trace!("[10] post() to {:?}", target_worker);
                                target_sem.post();
                                return TaskResult::Suspend;
                            }

                            Scheduler::terminate_thread(ctx, TerminationMethod::Cancellation)
                        },
                        fizzle_alloc(),
                    ),
                    YieldUntil::Reschedule(Duration::ZERO),
                )
            }
            ThreadCancelState::Finish => Outcome::Success(()),
        }
    }
}

pub struct ThreadCleanupPushEvent {
    routine: PTDestructor,
    arg: *mut libc::c_void,
}

impl ThreadCleanupPushEvent {
    pub fn new(routine: PTDestructor, arg: *mut libc::c_void) -> Self {
        Self { routine, arg }
    }
}

impl Event for ThreadCleanupPushEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state
            .local
            .pthread_cleanup
            .get_mut(&thread::current().id())
            .unwrap()
            .push_back(PThreadRoutine {
                function: self.routine,
                arg: Some(self.arg),
            });

        Outcome::Success(())
    }
}

pub struct ThreadCleanupPopEvent {
    execute: bool,
}

impl ThreadCleanupPopEvent {
    pub fn new(execute: bool) -> Self {
        Self { execute }
    }
}

impl Event for ThreadCleanupPopEvent {
    type Success = Option<PThreadRoutine>;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if let Some(routine) = state
            .local
            .pthread_cleanup
            .get_mut(&thread::current().id())
            .unwrap()
            .pop_front()
        {
            if self.execute {
                return Outcome::Success(Some(routine));
            }
        }
        Outcome::Success(None)
    }
}

pub struct ThreadKeyCreateEvent {
    key: *mut libc::pthread_key_t,
    destructor: PTDestructor,
}

impl ThreadKeyCreateEvent {
    pub fn new(key: *mut libc::pthread_key_t, destructor: PTDestructor) -> Self {
        Self { key, destructor }
    }
}

impl Event for ThreadKeyCreateEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let ret = unsafe { libc::pthread_key_create(self.key, None) };
        if ret == 0 {
            let key = unsafe { *self.key };
            state.local.pthread_keys.insert(
                key,
                PThreadRoutine {
                    function: self.destructor,
                    arg: None,
                },
            );
            state
                .local
                .pthread_key_values
                .insert(key, HashMap::with_hasher(Default::default()));

            Outcome::Success(())
        } else {
            Outcome::Error(Errno::get_errno())
        }
    }
}

pub struct ThreadKeyDeleteEvent {
    key: libc::pthread_key_t,
}

impl ThreadKeyDeleteEvent {
    pub fn new(key: libc::pthread_key_t) -> Self {
        Self { key }
    }
}

impl Event for ThreadKeyDeleteEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let ret = unsafe { libc::pthread_key_delete(self.key) };
        if ret == 0 {
            state.local.pthread_keys.remove(&self.key);
            state.local.pthread_key_values.remove(&self.key);

            Outcome::Success(())
        } else {
            Outcome::Error(Errno::get_errno())
        }
    }
}

pub enum ThreadYieldState {
    Start,
    Finish,
}

pub struct ThreadYieldEvent {
    state: ThreadYieldState,
}

impl ThreadYieldEvent {
    pub fn new() -> Self {
        Self {
            state: ThreadYieldState::Start,
        }
    }
}

impl Event for ThreadYieldEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            ThreadYieldState::Start => {
                state.mark_thread_ready(thread::current().id());
                self.state = ThreadYieldState::Finish;
                Outcome::Yield(YieldUntil::None)
            }
            ThreadYieldState::Finish => Outcome::Success(()),
        }
    }
}

pub struct ThreadCancellableEvent {
    cancellable: bool,
}

impl ThreadCancellableEvent {
    pub fn new(cancellable: bool) -> Self {
        Self { cancellable }
    }
}

impl Event for ThreadCancellableEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let pthread = unsafe { libc::pthread_self() };
        let thread_info = state.local.pthreads.get_mut(&pthread).unwrap();

        let prev_cancellable = thread_info.cancellable;
        thread_info.cancellable = self.cancellable;

        if thread_info.cancellable && thread_info.cancel_requested {
            Outcome::RunTask(
                Box::new_in(
                    |ctx| Scheduler::terminate_thread(ctx, TerminationMethod::Cancellation),
                    fizzle_alloc(),
                ),
                YieldUntil::Reschedule(Duration::ZERO),
            )
        } else {
            Outcome::Success(prev_cancellable)
        }
    }
}

pub struct ThreadCancelTypeEvent {
    cancel_type: ThreadCancelType,
}

impl ThreadCancelTypeEvent {
    pub fn new(cancel_type: ThreadCancelType) -> Self {
        Self { cancel_type }
    }
}

impl Event for ThreadCancelTypeEvent {
    type Success = ThreadCancelType;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let pthread = unsafe { libc::pthread_self() };
        let thread_info = state.local.pthreads.get_mut(&pthread).unwrap();

        let old_cancel_type = thread_info.cancel_type;
        thread_info.cancel_type = self.cancel_type;

        Outcome::Success(old_cancel_type)
    }
}

pub struct ThreadTestCancelEvent;

impl Event for ThreadTestCancelEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // Cancellation happens immediately in Fizzle, so this will always return
        Outcome::Success(())
    }
}

pub struct ThreadSetSpecificEvent {
    key: libc::pthread_key_t,
    pointer: *mut libc::c_void,
}

impl ThreadSetSpecificEvent {
    pub fn new(key: libc::pthread_key_t, pointer: *mut libc::c_void) -> Self {
        Self { key, pointer }
    }
}

impl Event for ThreadSetSpecificEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state
            .local
            .pthread_key_values
            .get_mut(&self.key)
            .unwrap()
            .insert(thread::current().id(), self.pointer);
        Outcome::Success(())
    }
}

pub struct ThreadGetSpecificEvent {
    key: libc::pthread_key_t,
}

impl ThreadGetSpecificEvent {
    pub fn new(key: libc::pthread_key_t) -> Self {
        Self { key }
    }
}

impl Event for ThreadGetSpecificEvent {
    type Success = *mut libc::c_void;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        Outcome::Success(
            *state
                .local
                .pthread_key_values
                .get_mut(&self.key)
                .unwrap()
                .get_mut(&thread::current().id())
                .unwrap_or(&mut ptr::null_mut()),
        )
    }
}
