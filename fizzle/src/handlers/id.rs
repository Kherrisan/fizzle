use std::thread::{self, ThreadId};

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use super::process::{Pgid, Pid};
use super::thread::Tid;


/// The unique identifying information for a given thread in a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Worker {
    pub pid: Pid,
    pub thread_id: ThreadId,
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
        Outcome::Success(state.local.process_info.borrow().pid.as_raw())
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
        Outcome::Success(state.local.process_info.borrow().ppid.as_raw())
    }
}

pub struct ProcessGetGroupIdEvent {
    pid: Option<Pid>,
}

impl ProcessGetGroupIdEvent {
    pub fn new(pid: Option<Pid>) -> Self {
        Self { pid }
    }
}

impl Event for ProcessGetGroupIdEvent {
    type Success = Pgid;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.pid {
            Some(pid) => match state.global.pids.get(&pid) {
                Some(process) => Outcome::Success(process.borrow().pgid),
                None => Outcome::Error(Errno::ESRCH),
            }
            None => Outcome::Success(state.local.process_info.borrow().pgid),
        }
        
    }
}

pub struct ProcessSetGroupIdEvent {
    pid: Option<Pid>,
    pgid: Pgid,
}

impl ProcessSetGroupIdEvent {
    pub fn new(pid: Option<Pid>, pgid: Pgid) -> Self {
        Self { pid, pgid }
    }
}

impl Event for ProcessSetGroupIdEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if let Some(pid) = self.pid {
            match state.global.process_groups.get(&self.pgid) {
                Some(group_pids) => if !group_pids.contains(&pid) {
                    return Outcome::Error(Errno::EPERM)  
                }
                None => return Outcome::Error(Errno::ESRCH),
            }

            // Setting the process group ID for another PID
            match state.global.pids.get(&pid) {
                Some(worker_info) => {
                    // TODO: check access control
                    worker_info.borrow_mut().pgid = self.pgid;
                    Outcome::Success(())
                }
                None => Outcome::Error(Errno::ESRCH),
            }

        } else {
            state.local.process_info.borrow_mut().pgid = self.pgid;
            Outcome::Success(())
        }
    }
}

pub struct ThreadGetIdEvent;

impl Event for ThreadGetIdEvent {
    type Success = Tid;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let tid = state.local.thread_tids.get(&thread::current().id()).unwrap();
        Outcome::Success(*tid)
    }
}
