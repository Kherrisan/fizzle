use std::thread::{self, ThreadId};

use crate::{scheduler::{Event, Outcome}, state::FizzleState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BarrierPtr(usize);

impl From<*mut libc::pthread_barrier_t> for BarrierPtr {
    fn from(value: *mut libc::pthread_barrier_t) -> Self {
        BarrierPtr(value as usize)
    }
}

#[derive(Debug)]
pub struct BarrierInfo {
    pub curr: Vec<ThreadId>,
    pub needed: usize,
}

impl BarrierInfo {
    pub fn new(count: usize) -> Self {
        Self {
            curr: Default::default(),
            needed: count,
        }
    }
}

pub struct BarrierInitEvent {
    barrier: BarrierPtr,
    count: usize,
}

impl BarrierInitEvent {
    pub fn new(barrier: BarrierPtr, count: usize) -> Self {
        Self {
            barrier,
            count,
        }
    }
}

impl Event for BarrierInitEvent {
    type Success = ();
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        if state.local.barriers.insert(self.barrier, BarrierInfo::new(self.count)).is_some() {
            panic!("[UB] `pthread_mutex_init()` called twice on one mutex");
        }

        Outcome::Success(())
    }
}

pub struct BarrierDestroyEvent {
    barrier: BarrierPtr,
}

impl BarrierDestroyEvent {
    pub fn new(barrier: BarrierPtr) -> Self {
        Self {
            barrier,
        }
    }
}

impl Event for BarrierDestroyEvent {
    type Success = ();
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        match state.local.barriers.remove(&self.barrier) {
            Some(barrier_info) if !barrier_info.curr.is_empty() => panic!("[UB] `pthread_barrier_destroy()` called on barrier other threads were waiting on"),
            None => panic!("[UB] `pthread_barrier_destroy()` called on uninitialized barrier"),
            _ => ()
        }

        Outcome::Success(())
    }
}

pub enum BarrierWaitState {
    Start,
    Finish,
}

pub struct BarrierWaitEvent {
    barrier: BarrierPtr,
    state: BarrierWaitState,
}

impl BarrierWaitEvent {
    pub fn new(barrier: BarrierPtr) -> Self {
        Self {
            barrier,
            state: BarrierWaitState::Start,
        }
    }
}

impl Event for BarrierWaitEvent {
    type Success = bool;
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            BarrierWaitState::Start => {
                self.state = BarrierWaitState::Finish;

                let Some(barrier_info) = state.local.barriers.get_mut(&self.barrier) else {
                    panic!("[UB] `pthread_barrier_wait` called on uninitialized barrier");
                };

                barrier_info.curr.push(thread::current().id());

                if barrier_info.curr.len() == barrier_info.needed {
                    // Release all threads (including this one)
                    let threads: Vec<ThreadId> = barrier_info.curr.drain(..).collect();
                    for thread_id in threads {
                        state.mark_thread_ready(thread_id);
                    }

                    Outcome::Success(true)

                } else {
                    Outcome::Yield(None)
                }
            }
            BarrierWaitState::Finish => Outcome::Success(false),
        }
    }
}
