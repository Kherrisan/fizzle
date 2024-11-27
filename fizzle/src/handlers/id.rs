use std::thread::{self, ThreadId};

use crate::{
    arena::ArenaKey,
    errno::Errno,
    scheduler::{Event, Outcome},
    state::FizzleState,
};

use super::process::{ProcessGroupId, ProcessId};

/// The ID associated with a given thread or process.
///
/// Equivalent to a pid or tid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct WorkerId(usize);

impl WorkerId {
    pub fn from_id(pid: libc::c_int) -> Self {
        debug_assert!(pid > 0);
        Self(pid as usize)
    }

    pub fn as_id(&self) -> libc::c_int {
        self.0 as i32
    }

    /// The PID corresponding to the primary process (e.g., PID 2).
    pub fn primary() -> Self {
        // pid 0 and pid 1 are special, so we start with 2
        Self(2)
    }

    /// The PID corresponding to the `init` process (e.g., PID 1).
    pub fn init_process() -> Self {
        Self(1)
    }
}

impl ArenaKey for WorkerId {
    type Value = WorkerInfo;
}

/// The unique identifying information for a given thread in a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkerInfo {
    pub process_id: ProcessId,
    pub thread_id: ThreadId,
}

impl WorkerInfo {
    /// Returns the current running worker.
    pub fn current(process_id: ProcessId) -> Self {
        Self {
            process_id,
            thread_id: thread::current().id(),
        }
    }
}

pub struct ProcessGetIdEvent;

impl ProcessGetIdEvent {
    pub fn new() -> Self {
        Self
    }
}

impl Event for ProcessGetIdEvent {
    type Success = libc::pid_t;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let process_id = state.local.process_id;
        Outcome::Success(state.global.processes.get(&process_id).unwrap().pid.as_id())
    }
}

pub struct ProcessGetParentIdEvent;

impl ProcessGetParentIdEvent {
    pub fn new() -> Self {
        Self
    }
}

impl Event for ProcessGetParentIdEvent {
    type Success = libc::pid_t;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let process_id = state.local.process_id;
        Outcome::Success(
            state
                .global
                .processes
                .get(&process_id)
                .unwrap()
                .ppid
                .as_id(),
        )
    }
}

pub struct ProcessGetGroupIdEvent {
    pid: Option<WorkerId>,
}

impl ProcessGetGroupIdEvent {
    pub fn new(pid: Option<WorkerId>) -> Self {
        Self { pid }
    }
}

impl Event for ProcessGetGroupIdEvent {
    type Success = ProcessGroupId;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let process_id = match self.pid {
            Some(pid) => state.global.ids.get(&pid).unwrap().process_id,
            None => state.local.process_id,
        };
        Outcome::Success(state.global.processes.get(&process_id).unwrap().pgid)
    }
}

pub struct ProcessSetGroupIdEvent {
    pid: Option<WorkerId>,
    pgid: ProcessGroupId,
}

impl ProcessSetGroupIdEvent {
    pub fn new(pid: Option<WorkerId>, pgid: ProcessGroupId) -> Self {
        Self { pid, pgid }
    }
}

impl Event for ProcessSetGroupIdEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let process_id = match self.pid {
            Some(pid) => state.global.ids.get(&pid).unwrap().process_id,
            None => state.local.process_id,
        };

        let Some(process_info) = state.global.processes.get_mut(&process_id) else {
            return Outcome::Error(Errno::ESRCH);
        };

        if !state.global.process_groups.is_occupied(&self.pgid) {
            return Outcome::Error(Errno::EPERM);
        }

        let old_pgid = process_info.pgid;
        process_info.pgid = self.pgid;

        state
            .global
            .process_groups
            .get_mut(&old_pgid)
            .unwrap()
            .remove(&process_id);
        state
            .global
            .process_groups
            .get_mut(&self.pgid)
            .unwrap()
            .insert(process_id);

        Outcome::Success(())
    }
}

pub struct ThreadGetIdEvent;

impl Event for ThreadGetIdEvent {
    type Success = WorkerId;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let tid = state.local.tids.get(&thread::current().id()).unwrap();
        Outcome::Success(*tid)
    }
}
