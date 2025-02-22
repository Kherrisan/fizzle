use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::os::fd::RawFd;
use std::rc::Rc;
use std::time::Duration;

use crate::backend::{ConnectedBackend, ConnectionlessBackend, StdioBackend};
use crate::errno::Errno;
use crate::handlers::epoll::{EpollDirection, EpollInterest};
use crate::scheduler::{fizzle_alloc, Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::{GlobalRc, GlobalSet, GlobalVec};

use fxhash::FxBuildHasher;

use super::descriptor::{Descriptor, DescriptorInfo, FdResource};
use super::epoll::{EpollInfo, PolledStatus};
use super::id::Worker;
use super::polled::PolledInfo;
use super::signal::{SigmaskOp, SignalSet, SignalSetSigmaskEvent};
use super::socket::SocketState;

pub struct PollerInfo {
    pub worker: Worker,
    pub polled_events: GlobalVec<GlobalRc<PolledInfo>>,
    /// Polled events that have been raised for the Poller prior to it being evaluated.
    ///
    /// A poller will have raised events if and only if it is in the ready_queue; this invariant is
    /// reflected in the `in_raised_queue()` method defined below.
    pub raised_events: GlobalSet<GlobalRc<PolledInfo>>,
}

impl PollerInfo {
    pub fn in_raised_queue(&self) -> bool {
        !self.raised_events.is_empty()
    }
}

impl PartialEq for PollerInfo {
    fn eq(&self, other: &Self) -> bool {
        self.worker == other.worker
    }
}

impl Eq for PollerInfo {}

enum SelectState {
    Start,
    ApplySigmask(SignalSetSigmaskEvent),
    CheckDescriptors,
    CheckDescriptorsFail(Errno),
    EndPoll(
        GlobalRc<PollerInfo>,
        HashMap<RawFd, GlobalRc<PolledInfo>, FxBuildHasher>,
        HashMap<RawFd, GlobalRc<PolledInfo>, FxBuildHasher>,
    ),
    RevertSigmask(SignalSetSigmaskEvent, Result<usize, Errno>),
}

pub struct SelectEvent<'a> {
    nfds: usize,
    readfds: Option<&'a mut libc::fd_set>,
    writefds: Option<&'a mut libc::fd_set>,
    exceptfds: Option<&'a mut libc::fd_set>,
    timeout: Option<Duration>,
    sigmask: Option<SignalSet>,
    state: SelectState,
}

impl<'a> SelectEvent<'a> {
    pub fn new(
        nfds: usize,
        readfds: Option<&'a mut libc::fd_set>,
        writefds: Option<&'a mut libc::fd_set>,
        exceptfds: Option<&'a mut libc::fd_set>,
        timeout: Option<Duration>,
        sigmask: Option<SignalSet>,
    ) -> Self {
        // TODO: temporary workaround for certain programs that loop on a zero timeout duration
        let timeout = match timeout {
            Some(t) if t == Duration::ZERO => Some(Duration::from_secs(5)),
            t => t,
        };

        Self {
            nfds,
            readfds,
            writefds,
            exceptfds,
            timeout,
            sigmask,
            state: SelectState::Start,
        }
    }
}

impl Event for SelectEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            SelectState::Start => {
                if let Some(sigmask) = self.sigmask {
                    self.state = SelectState::ApplySigmask(SignalSetSigmaskEvent::new(
                        SigmaskOp::Setmask,
                        Some(sigmask),
                    ));
                } else {
                    self.state = SelectState::CheckDescriptors;
                }
                Outcome::Yield(YieldUntil::Immediate)
            }
            SelectState::ApplySigmask(event) => {
                match event.run(state) {
                    Outcome::Success(s) => {
                        // Store the replacement signal mask to revert state to
                        self.sigmask = Some(s);
                        self.state = SelectState::CheckDescriptors;
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Error(_) => unreachable!(), // Errors shouldn't happen in this event
                    // For all other outcomes, have the scheduler continue running
                    Outcome::Yield(until) => Outcome::Yield(until),
                    Outcome::RunTask(t, y) => Outcome::RunTask(t, y),
                }
            }
            SelectState::CheckDescriptors => {
                let mut total_ready = 0;
                if let Some(exceptfds) = &mut self.exceptfds {
                    unsafe { libc::FD_ZERO(*exceptfds) };
                }

                let mut read_pollers = HashMap::with_hasher(FxBuildHasher::default());
                let mut write_pollers = HashMap::with_hasher(FxBuildHasher::default());

                for fd in 0..self.nfds as libc::c_int {
                    let mut fd_ready = false;
                    if let Some(readfds) = &mut self.readfds {
                        if unsafe { libc::FD_ISSET(fd, *readfds) } {
                            match fd_to_pollin(state, fd) {
                                PolledStatus::Pollable(polled) => {
                                    if !state.polled_is_ready(&polled) {
                                        log::trace!("select(): fd {} was set for reading (Pollable | NotReady)", fd);
                                        unsafe { libc::FD_CLR(fd, *readfds) };
                                        read_pollers.insert(fd, polled.clone());
                                    } else {
                                        log::trace!("select(): fd {} was set for reading (Pollable | Ready)", fd);
                                        fd_ready = true;
                                    }
                                }
                                PolledStatus::BadFd => {
                                    log::warn!("select(): fd {} in readfds was not recognized (returning EBADF)", fd);
                                    self.state = SelectState::CheckDescriptorsFail(Errno::EBADF);
                                    return Outcome::Yield(YieldUntil::Immediate);
                                }
                                PolledStatus::NotPollable => {
                                    log::trace!(
                                        "select(): fd {} was set for reading (NotPollable)",
                                        fd
                                    );
                                    unsafe {
                                        libc::FD_CLR(fd, *readfds);
                                    }
                                }
                                PolledStatus::ImmediatelyPollable => {
                                    log::trace!(
                                        "select(): fd {} was set for reading (ImmediatelyPollable)",
                                        fd
                                    );
                                    fd_ready = true;
                                }
                            }
                        }
                    }

                    if let Some(writefds) = &mut self.writefds {
                        if unsafe { libc::FD_ISSET(fd, *writefds) } {
                            match fd_to_pollout(state, fd) {
                                PolledStatus::Pollable(polled_id) => {
                                    if !state.polled_is_ready(&polled_id) {
                                        log::trace!("`select`: fd {} was set for reading (Pollable | NotReady)", fd);
                                        unsafe {
                                            libc::FD_CLR(fd, *writefds);
                                        }
                                        write_pollers.insert(fd, polled_id);
                                    } else {
                                        log::trace!("`select`: fd {} was set for writing (Pollable | Ready)", fd);
                                        fd_ready = true;
                                    }
                                }
                                PolledStatus::BadFd => {
                                    log::warn!("`select`: fd {} in writefds was not recognized (returning EBADF)", fd);
                                    self.state = SelectState::CheckDescriptorsFail(Errno::EBADF);
                                    return Outcome::Yield(YieldUntil::Immediate);
                                }
                                PolledStatus::NotPollable => {
                                    log::trace!(
                                        "`select`: fd {} was set for writing (NotPollable)",
                                        fd
                                    );
                                    unsafe {
                                        libc::FD_CLR(fd, *writefds);
                                    }
                                }
                                PolledStatus::ImmediatelyPollable => {
                                    log::trace!(
                                        "`select`: fd {} was set for writing (ImmediatelyPollable)",
                                        fd
                                    );
                                    fd_ready = true;
                                }
                            }
                        }
                    }

                    if fd_ready {
                        total_ready += 1;
                    }
                }

                if total_ready > 0 || self.timeout == Some(Duration::ZERO) {
                    return Outcome::Success(total_ready);
                }

                let poller_id = state.new_poller();

                let all_pollers: BTreeSet<GlobalRc<PolledInfo>> = read_pollers
                    .clone()
                    .into_iter()
                    .chain(write_pollers.clone())
                    .map(|(_fd, polled)| polled)
                    .collect();
                for polled_id in all_pollers {
                    state.register_poller(poller_id.clone(), polled_id);
                }

                self.state = SelectState::EndPoll(poller_id, read_pollers, write_pollers);
                Outcome::Yield(match self.timeout {
                    Some(timeout) => YieldUntil::Reschedule(timeout),
                    None => YieldUntil::None,
                })
            }
            SelectState::CheckDescriptorsFail(e) => match self.sigmask {
                Some(sigmask) => {
                    self.state = SelectState::RevertSigmask(
                        SignalSetSigmaskEvent::new(SigmaskOp::Setmask, Some(sigmask)),
                        Err(*e),
                    );
                    Outcome::Yield(YieldUntil::Immediate)
                }
                None => Outcome::Error(*e),
            },
            SelectState::EndPoll(poller_id, read_pollers, write_pollers) => {
                state.delete_poller(poller_id.clone());

                let mut total_ready = 0;

                for fd in 0..self.nfds as RawFd {
                    let mut fd_is_ready = false;

                    if let Some(readfds) = &mut self.readfds {
                        if let Some(polled_id) = read_pollers.get(&fd) {
                            if state.polled_is_ready(polled_id) {
                                unsafe {
                                    libc::FD_SET(fd, *readfds);
                                }
                                fd_is_ready = true;
                            }
                        }
                    }

                    if let Some(writefds) = &mut self.writefds {
                        if let Some(polled_id) = write_pollers.get(&fd) {
                            if state.polled_is_ready(polled_id) {
                                unsafe {
                                    libc::FD_SET(fd, *writefds);
                                }
                                fd_is_ready = true;
                            }
                        }
                    }

                    if fd_is_ready {
                        total_ready += 1;
                    }
                }

                match self.sigmask {
                    Some(sigmask) => {
                        self.state = SelectState::RevertSigmask(
                            SignalSetSigmaskEvent::new(SigmaskOp::Setmask, Some(sigmask)),
                            Ok(total_ready),
                        );
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    None => Outcome::Success(total_ready),
                }
            }
            SelectState::RevertSigmask(event, res) => {
                match event.run(state) {
                    Outcome::Success(s) => {
                        // Store the replacement signal mask to revert state to
                        self.sigmask = Some(s);

                        match res {
                            Ok(s) => Outcome::Success(*s),
                            Err(e) => Outcome::Error(*e),
                        }
                    }
                    Outcome::Error(_) => unreachable!(), // Errors shouldn't happen in this event
                    // For all other outcomes, have the scheduler continue running
                    Outcome::Yield(duration) => Outcome::Yield(duration),
                    Outcome::RunTask(t, y) => Outcome::RunTask(t, y),
                }
            }
        }
    }
}

enum PollState {
    Start,
    ApplySigmask(SignalSetSigmaskEvent),
    CheckDescriptors,
    CheckDescriptorsFail(Errno),
    EndPoll(
        GlobalRc<PollerInfo>,
        HashMap<RawFd, GlobalRc<PolledInfo>, FxBuildHasher>,
        HashMap<RawFd, GlobalRc<PolledInfo>, FxBuildHasher>,
    ),
    RevertSigmask(SignalSetSigmaskEvent, Result<usize, Errno>),
}

pub struct PollEvent<'a> {
    fd_info: &'a mut [libc::pollfd],
    timeout: Option<Duration>,
    sigmask: Option<SignalSet>,
    state: PollState,
}

impl<'a> PollEvent<'a> {
    pub fn new(
        fd_info: &'a mut [libc::pollfd],
        timeout: Option<Duration>,
        sigmask: Option<SignalSet>,
    ) -> Self {
        Self {
            fd_info,
            timeout,
            sigmask,
            state: PollState::Start,
        }
    }
}

impl Event for PollEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            PollState::Start => {
                if let Some(sigmask) = self.sigmask {
                    self.state = PollState::ApplySigmask(SignalSetSigmaskEvent::new(
                        SigmaskOp::Setmask,
                        Some(sigmask),
                    ));
                } else {
                    self.state = PollState::CheckDescriptors;
                }
                Outcome::Yield(YieldUntil::Immediate)
            }
            PollState::ApplySigmask(event) => {
                match event.run(state) {
                    Outcome::Success(s) => {
                        // Store the replacement signal mask to revert state to
                        self.sigmask = Some(s);
                        self.state = PollState::CheckDescriptors;
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Error(_) => unreachable!(), // Errors shouldn't happen in this event
                    // For all other outcomes, have the scheduler continue running
                    Outcome::Yield(duration) => Outcome::Yield(duration),
                    Outcome::RunTask(t, y) => Outcome::RunTask(t, y),
                }
            }
            PollState::CheckDescriptors => {
                let mut total_ready = 0;

                let mut read_pollers = HashMap::with_hasher(FxBuildHasher::default());
                let mut write_pollers = HashMap::with_hasher(FxBuildHasher::default());

                for pfd in self.fd_info.iter_mut() {
                    let mut fd_ready = false;
                    let fd = pfd.fd;
                    let events = pfd.events;
                    pfd.revents = 0; // Assign 0 by default until filled in later

                    if events & libc::POLLIN != 0 {
                        match fd_to_pollin(state, fd) {
                            PolledStatus::Pollable(polled_id) => {
                                if !state.polled_is_ready(&polled_id) {
                                    log::trace!(
                                        "`poll`: fd {} was set for reading (Pollable | NotReady)",
                                        fd
                                    );
                                    read_pollers.insert(fd, polled_id);
                                } else {
                                    log::trace!(
                                        "`poll`: fd {} was set for reading (Pollable | Ready)",
                                        fd
                                    );
                                    fd_ready = true;
                                }
                            }
                            PolledStatus::BadFd => {
                                log::warn!(
                                    "`poll`: fd {} was not recognized (returning EBADF)",
                                    fd
                                );
                                self.state = PollState::CheckDescriptorsFail(Errno::EBADF);
                                return Outcome::Yield(YieldUntil::Immediate);
                            }
                            PolledStatus::NotPollable => {
                                log::trace!("`poll`: fd {} was set for reading (NotPollable)", fd);
                            }
                            PolledStatus::ImmediatelyPollable => {
                                log::trace!(
                                    "`poll`: fd {} was set for reading (ImmediatelyPollable)",
                                    fd
                                );
                                fd_ready = true;
                            }
                        }
                    }

                    if events & libc::POLLOUT != 0 {
                        match fd_to_pollout(state, fd) {
                            PolledStatus::Pollable(polled_id) => {
                                if !state.polled_is_ready(&polled_id) {
                                    log::trace!(
                                        "`poll`: fd {} was set for reading (Pollable | NotReady)",
                                        fd
                                    );
                                    write_pollers.insert(fd, polled_id);
                                } else {
                                    log::trace!(
                                        "`poll`: fd {} was set for writing (Pollable | Ready)",
                                        fd
                                    );
                                    fd_ready = true;
                                }
                            }
                            PolledStatus::BadFd => {
                                log::warn!("`poll`: fd {} in writefds was not recognized (returning EBADF)", fd);
                                self.state = PollState::CheckDescriptorsFail(Errno::EBADF);
                                return Outcome::Yield(YieldUntil::Immediate);
                            }
                            PolledStatus::NotPollable => {
                                log::trace!("`poll`: fd {} was set for writing (NotPollable)", fd);
                            }
                            PolledStatus::ImmediatelyPollable => {
                                log::trace!(
                                    "`poll`: fd {} was set for writing (ImmediatelyPollable)",
                                    fd
                                );
                                fd_ready = true;
                            }
                        }
                    }

                    if fd_ready {
                        total_ready += 1;
                    }
                }

                if total_ready > 0 || self.timeout == Some(Duration::ZERO) {
                    return Outcome::Success(total_ready);
                }

                let poller_id = state.new_poller();

                let all_pollers: BTreeSet<GlobalRc<PolledInfo>> = read_pollers
                    .clone()
                    .into_iter()
                    .chain(write_pollers.clone())
                    .map(|(_fd, polled)| polled)
                    .collect();
                for polled_id in all_pollers {
                    state.register_poller(poller_id.clone(), polled_id);
                }

                self.state = PollState::EndPoll(poller_id, read_pollers, write_pollers);
                Outcome::Yield(match self.timeout {
                    Some(timeout) => YieldUntil::Reschedule(timeout),
                    None => YieldUntil::None,
                })
            }
            PollState::CheckDescriptorsFail(e) => match self.sigmask {
                Some(sigmask) => {
                    self.state = PollState::RevertSigmask(
                        SignalSetSigmaskEvent::new(SigmaskOp::Setmask, Some(sigmask)),
                        Err(*e),
                    );
                    Outcome::Yield(YieldUntil::Immediate)
                }
                None => Outcome::Error(*e),
            },
            PollState::EndPoll(poller_id, read_pollers, write_pollers) => {
                state.delete_poller(poller_id.clone());

                let mut total_ready = 0;

                for pfd in self.fd_info.iter_mut() {
                    let mut fd_is_ready = false;
                    let fd = pfd.fd;
                    //pfd.revents

                    if let Some(polled_id) = read_pollers.get(&fd) {
                        if state.polled_is_ready(polled_id) {
                            pfd.revents |= libc::POLLIN;
                            fd_is_ready = true;
                        }
                    }

                    if let Some(polled_id) = write_pollers.get(&fd) {
                        if state.polled_is_ready(polled_id) {
                            pfd.revents |= libc::POLLOUT;
                            fd_is_ready = true;
                        }
                    }

                    if fd_is_ready {
                        total_ready += 1;
                    }
                }

                match self.sigmask {
                    Some(sigmask) => {
                        self.state = PollState::RevertSigmask(
                            SignalSetSigmaskEvent::new(SigmaskOp::Setmask, Some(sigmask)),
                            Ok(total_ready),
                        );
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    None => Outcome::Success(total_ready),
                }
            }
            PollState::RevertSigmask(event, res) => {
                match event.run(state) {
                    Outcome::Success(s) => {
                        // Store the replacement signal mask to revert state to
                        self.sigmask = Some(s);

                        match res {
                            Ok(s) => Outcome::Success(*s),
                            Err(e) => Outcome::Error(*e),
                        }
                    }
                    Outcome::Error(_) => unreachable!(), // Errors shouldn't happen in this event
                    // For all other outcomes, have the scheduler continue running
                    Outcome::Yield(until) => Outcome::Yield(until),
                    Outcome::RunTask(t, y) => Outcome::RunTask(t, y),
                }
            }
        }
    }
}

pub struct EpollCreateEvent {
    cloexec: bool,
}

impl EpollCreateEvent {
    pub fn new(cloexec: bool) -> Self {
        Self { cloexec }
    }
}

impl Event for EpollCreateEvent {
    type Success = Descriptor;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let fd = Descriptor::from_raw_fd(crate::create_descriptor());
        let epoll = Rc::new_in(
            RefCell::new(EpollInfo {
                interests: BTreeMap::new_in(fizzle_alloc()),
            }),
            fizzle_alloc(),
        );

        state.local.fds.insert(
            fd,
            DescriptorInfo {
                close_on_exec: self.cloexec,
                nonblocking: false,
                is_passthrough: false,
                resource: FdResource::Epoll(epoll),
            },
        );

        Outcome::Success(fd)
    }
}

#[derive(Clone, Copy)]
pub enum EpollOperation {
    Add(libc::epoll_event),
    Delete,
    Modify(libc::epoll_event),
}

pub struct EpollCtlEvent {
    epoll_descriptor: Descriptor,
    op: EpollOperation,
    target_descriptor: Descriptor,
}

impl EpollCtlEvent {
    pub fn new(
        epoll_descriptor: Descriptor,
        op: EpollOperation,
        target_descriptor: Descriptor,
    ) -> Self {
        Self {
            epoll_descriptor,
            op,
            target_descriptor,
        }
    }
}

impl Event for EpollCtlEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if self.epoll_descriptor == self.target_descriptor {
            log::error!("epoll descriptor attempted to poll itself");
            return Outcome::Error(Errno::EINVAL);
        }

        let Some(epfd_info) = state.local.fds.get(&self.epoll_descriptor) else {
            return Outcome::Error(Errno::EBADF);
        };

        let FdResource::Epoll(epoll) = epfd_info.resource.clone() else {
            return Outcome::Error(Errno::EINVAL);
        };

        let Some(_) = state.local.fds.get(&self.target_descriptor) else {
            log::error!(
                "`epoll_ctl` fd {} not found",
                self.target_descriptor.as_raw_fd()
            );
            return Outcome::Error(Errno::EBADF);
            // TODO: this used to ignore rather than erroring to handle a magma issue
        };

        match self.op {
            EpollOperation::Add(ev) => {
                let target_fd = self.target_descriptor.as_raw_fd();
                if epoll
                    .borrow()
                    .interests
                    .contains_key(&self.target_descriptor)
                {
                    return Outcome::Error(Errno::EEXIST);
                }

                let mut read_status = None;
                let mut write_status = None;

                if (ev.events & libc::EPOLLIN as u32) != 0 {
                    read_status = Some(fd_to_pollin(state, target_fd));
                }

                if (ev.events & libc::EPOLLOUT as u32) != 0 {
                    write_status = Some(fd_to_pollout(state, target_fd));
                }

                let direction = match (read_status, write_status) {
                    (None, None) => EpollDirection::None,
                    (Some(status), None) => EpollDirection::Read(status),
                    (None, Some(status)) => EpollDirection::Write(status),
                    (Some(read_status), Some(write_status)) => {
                        EpollDirection::Both(read_status, write_status)
                    }
                };

                epoll.borrow_mut().interests.insert(
                    self.target_descriptor,
                    EpollInterest {
                        direction: direction.clone(),
                        user_data: ev.u64,
                    },
                );

                log::trace!(
                    "EPOLL_CTL_ADD called on epoll_fd({}) for fd({})--setting poll mode to {}",
                    self.epoll_descriptor.as_raw_fd(),
                    target_fd,
                    match direction {
                        EpollDirection::None => "NONE",
                        EpollDirection::Read(_) => "EPOLLIN",
                        EpollDirection::Write(_) => "EPOLLOUT",
                        EpollDirection::Both(_, _) => "EPOLLIN | EPOLLOUT",
                    }
                );

                Outcome::Success(())
            }
            EpollOperation::Delete => {
                if let Some(_) = epoll.borrow_mut().interests.remove(&self.target_descriptor) {
                    Outcome::Success(())
                } else {
                    Outcome::Error(Errno::ENOENT)
                }
            }
            EpollOperation::Modify(ev) => {
                let mut read_status = None;
                let mut write_status = None;

                if (ev.events & libc::EPOLLIN as u32) != 0 {
                    read_status = Some(fd_to_pollin(state, self.target_descriptor.as_raw_fd()));
                }

                if (ev.events & libc::EPOLLOUT as u32) != 0 {
                    write_status = Some(fd_to_pollout(state, self.target_descriptor.as_raw_fd()));
                }

                let direction = match (read_status, write_status) {
                    (None, None) => EpollDirection::None,
                    (Some(status), None) => EpollDirection::Read(status),
                    (None, Some(status)) => EpollDirection::Write(status),
                    (Some(read_status), Some(write_status)) => {
                        EpollDirection::Both(read_status, write_status)
                    }
                };

                let mut epoll_mut = epoll.borrow_mut();
                let Some(interest) = epoll_mut.interests.get_mut(&self.target_descriptor) else {
                    return Outcome::Error(Errno::ENOENT);
                };

                interest.direction = direction;
                interest.user_data = ev.u64;

                Outcome::Success(())
            }
        }
    }
}

enum EpollWaitState {
    Start,
    ApplySigmask(SignalSetSigmaskEvent),
    CheckDescriptors,
    CheckDescriptorsFail(Errno),
    EndPoll(
        GlobalRc<PollerInfo>,
        HashMap<RawFd, GlobalRc<PolledInfo>, FxBuildHasher>,
        HashMap<RawFd, GlobalRc<PolledInfo>, FxBuildHasher>,
    ),
    RevertSigmask(SignalSetSigmaskEvent, Result<usize, Errno>),
}

pub struct EpollWaitEvent<'a> {
    epoll_descriptor: Descriptor,
    events: &'a mut [libc::epoll_event],
    timeout: Option<Duration>,
    sigmask: Option<SignalSet>,
    state: EpollWaitState,
}

impl<'a> EpollWaitEvent<'a> {
    pub fn new(
        epoll_descriptor: Descriptor,
        events: &'a mut [libc::epoll_event],
        timeout: Option<Duration>,
        sigmask: Option<SignalSet>,
    ) -> Self {
        Self {
            epoll_descriptor,
            events,
            timeout,
            sigmask,
            state: EpollWaitState::Start,
        }
    }
}

impl Event for EpollWaitEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            EpollWaitState::Start => {
                if self.events.is_empty() {
                    return Outcome::Error(Errno::EINVAL);
                }

                if let Some(sigmask) = self.sigmask {
                    self.state = EpollWaitState::ApplySigmask(SignalSetSigmaskEvent::new(
                        SigmaskOp::Setmask,
                        Some(sigmask),
                    ));
                } else {
                    self.state = EpollWaitState::CheckDescriptors;
                }
                Outcome::Yield(YieldUntil::Immediate)
            }
            EpollWaitState::ApplySigmask(event) => {
                match event.run(state) {
                    Outcome::Success(s) => {
                        // Store the replacement signal mask to revert state to
                        self.sigmask = Some(s);
                        self.state = EpollWaitState::CheckDescriptors;
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Error(_) => unreachable!(), // Errors shouldn't happen in this event
                    // For all other outcomes, have the scheduler continue running
                    Outcome::Yield(u) => Outcome::Yield(u),
                    Outcome::RunTask(t, y) => Outcome::RunTask(t, y),
                }
            }
            EpollWaitState::CheckDescriptors => {
                let mut total_ready = 0;

                // TODO: BUG: ApplySigmask is called before this, but `return Outcome::Error` shortcuts
                // don't revert that sigmask...

                let mut read_pollers = HashMap::with_hasher(FxBuildHasher::default());
                let mut write_pollers = HashMap::with_hasher(FxBuildHasher::default());

                let Some(epfd_info) = state.local.fds.get(&self.epoll_descriptor) else {
                    return Outcome::Error(Errno::EBADF);
                };

                let FdResource::Epoll(epoll) = epfd_info.resource.clone() else {
                    return Outcome::Error(Errno::EINVAL);
                };

                for (target_descriptor, interest) in epoll.borrow().interests.iter() {
                    let mut fd_ready = false;
                    let fd = target_descriptor.as_raw_fd();
                    let direction = &interest.direction;
                    let mut events = 0;

                    if let EpollDirection::Read(status) | EpollDirection::Both(status, _) =
                        direction
                    {
                        match status {
                            PolledStatus::Pollable(polled) => {
                                if !state.polled_is_ready(polled) {
                                    log::trace!(
                                        "`poll`: fd {} was set for reading (Pollable | NotReady)",
                                        fd
                                    );
                                    read_pollers.insert(fd, polled.clone());
                                } else {
                                    log::trace!(
                                        "`poll`: fd {} was set for reading (Pollable | Ready)",
                                        fd
                                    );
                                    fd_ready = true;
                                    events |= libc::EPOLLIN;
                                }
                            }
                            PolledStatus::BadFd => {
                                log::warn!(
                                    "`poll`: fd {} was not recognized (returning EBADF)",
                                    fd
                                );
                                self.state = EpollWaitState::CheckDescriptorsFail(Errno::EBADF);
                                return Outcome::Yield(YieldUntil::Immediate);
                            }
                            PolledStatus::NotPollable => {
                                log::trace!("`poll`: fd {} was set for reading (NotPollable)", fd);
                            }
                            PolledStatus::ImmediatelyPollable => {
                                log::trace!(
                                    "`poll`: fd {} was set for reading (ImmediatelyPollable)",
                                    fd
                                );
                                fd_ready = true;
                                events |= libc::EPOLLIN;
                            }
                        }
                    }

                    if let EpollDirection::Write(status) | EpollDirection::Both(_, status) =
                        direction
                    {
                        match status {
                            PolledStatus::Pollable(polled_id) => {
                                if !state.polled_is_ready(polled_id) {
                                    log::trace!(
                                        "`poll`: fd {} was set for reading (Pollable | NotReady)",
                                        fd
                                    );
                                    write_pollers.insert(fd, polled_id.clone());
                                } else {
                                    log::trace!(
                                        "`poll`: fd {} was set for writing (Pollable | Ready)",
                                        fd
                                    );
                                    fd_ready = true;
                                    events |= libc::EPOLLOUT;
                                }
                            }
                            PolledStatus::BadFd => {
                                log::warn!("`poll`: fd {} in writefds was not recognized (returning EBADF)", fd);
                                self.state = EpollWaitState::CheckDescriptorsFail(Errno::EBADF);
                                return Outcome::Yield(YieldUntil::Immediate);
                            }
                            PolledStatus::NotPollable => {
                                log::trace!("`poll`: fd {} was set for writing (NotPollable)", fd);
                            }
                            PolledStatus::ImmediatelyPollable => {
                                log::trace!(
                                    "`poll`: fd {} was set for writing (ImmediatelyPollable)",
                                    fd
                                );
                                fd_ready = true;
                                events |= libc::EPOLLOUT;
                            }
                        }
                    }

                    if fd_ready && total_ready < self.events.len() {
                        self.events[total_ready].u64 = interest.user_data;
                        self.events[total_ready].events = events as u32;
                        total_ready += 1;
                    }
                }

                if total_ready > 0 || self.timeout == Some(Duration::ZERO) {
                    return Outcome::Success(total_ready);
                }

                let poller_id = state.new_poller();

                let all_pollers: BTreeSet<GlobalRc<PolledInfo>> = read_pollers
                    .clone()
                    .into_iter()
                    .chain(write_pollers.clone())
                    .map(|(_fd, polled)| polled)
                    .collect();
                for polled_id in all_pollers {
                    state.register_poller(poller_id.clone(), polled_id);
                }

                self.state = EpollWaitState::EndPoll(poller_id, read_pollers, write_pollers);
                Outcome::Yield(match self.timeout {
                    Some(timeout) => YieldUntil::Reschedule(timeout),
                    None => YieldUntil::None,
                })
            }
            EpollWaitState::CheckDescriptorsFail(e) => match self.sigmask {
                Some(sigmask) => {
                    self.state = EpollWaitState::RevertSigmask(
                        SignalSetSigmaskEvent::new(SigmaskOp::Setmask, Some(sigmask)),
                        Err(*e),
                    );
                    Outcome::Yield(YieldUntil::Immediate)
                }
                None => Outcome::Error(*e),
            },
            EpollWaitState::EndPoll(poller_id, read_pollers, write_pollers) => {
                state.delete_poller(poller_id.clone());

                let mut total_ready = 0;

                let Some(epfd_info) = state.local.fds.get(&self.epoll_descriptor) else {
                    return Outcome::Error(Errno::EBADF);
                };

                let FdResource::Epoll(epoll) = epfd_info.resource.clone() else {
                    return Outcome::Error(Errno::EINVAL);
                };


                for (target_descriptor, interest) in epoll.borrow().interests.iter() {
                    let mut fd_is_ready = false;
                    let fd = target_descriptor.as_raw_fd();
                    let mut events = 0;

                    if let Some(polled_id) = read_pollers.get(&fd) {
                        if state.polled_is_ready(polled_id) {
                            fd_is_ready = true;
                            events |= libc::EPOLLIN;
                        }
                    }

                    if let Some(polled_id) = write_pollers.get(&fd) {
                        if state.polled_is_ready(polled_id) {
                            fd_is_ready = true;
                            events |= libc::EPOLLOUT;
                        }
                    }

                    if fd_is_ready && total_ready < self.events.len() {
                        self.events[total_ready].u64 = interest.user_data;
                        self.events[total_ready].events = events as u32;
                        total_ready += 1;
                    }
                }

                match self.sigmask {
                    Some(sigmask) => {
                        self.state = EpollWaitState::RevertSigmask(
                            SignalSetSigmaskEvent::new(SigmaskOp::Setmask, Some(sigmask)),
                            Ok(total_ready),
                        );
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    None => Outcome::Success(total_ready),
                }
            }
            EpollWaitState::RevertSigmask(event, res) => {
                match event.run(state) {
                    Outcome::Success(s) => {
                        // Store the replacement signal mask to revert state to
                        self.sigmask = Some(s);

                        match res {
                            Ok(s) => Outcome::Success(*s),
                            Err(e) => Outcome::Error(*e),
                        }
                    }
                    Outcome::Error(_) => unreachable!(), // Errors shouldn't happen in this event
                    // For all other outcomes, have the scheduler continue running
                    Outcome::Yield(y) => Outcome::Yield(y),
                    Outcome::RunTask(t, y) => Outcome::RunTask(t, y),
                }
            }
        }
    }
}

/// Polled for read() operations
pub fn fd_to_pollin(state: &mut FizzleState, fd: RawFd) -> PolledStatus {
    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(fd)) else {
        return PolledStatus::BadFd;
    };
    match &fd_info.resource {
        FdResource::Epoll(_) => panic!("polling an epoll descriptor not supported"),
        FdResource::EventFd(eventfd) => {
            PolledStatus::Pollable(eventfd.borrow().read_polled.clone())
        }
        FdResource::Directory(_) => PolledStatus::NotPollable,
        FdResource::File(_file_id) => PolledStatus::ImmediatelyPollable, // Polling a file is not generally supported
        /*
        match &state.global.files.get(&file_id).unwrap().backend {
            FileBackend::Passthrough => PolledStatus::ImmediatelyPollable,
            FileBackend::Peered(_) => unreachable!(),
            FileBackend::Feedback(feedback) => PolledStatus::Pollable(feedback.read_polled.clone()),
            FileBackend::Plugin(plugin_id) => PolledStatus::Pollable(
                state
                    .global
                    .plugins
                    .get(plugin_id)
                    .unwrap()
                    .read_polled
                    .clone(),
            ),
            FileBackend::Sink => PolledStatus::NotPollable,
            FileBackend::NullSink => PolledStatus::ImmediatelyPollable,
            FileBackend::Fuzz(fuzz_endpoint_id) => PolledStatus::Pollable(
                state
                    .global
                    .fuzz_endpoints
                    .get(&fuzz_endpoint_id)
                    .unwrap()
                    .read_polled
                    .clone(),
            ),
        },
        */
        FdResource::MessageQueue(_) => todo!(),
        FdResource::Pipe(pipe_info) => {
            PolledStatus::Pollable(pipe_info.borrow().read_polled.clone())
        }
        FdResource::Stdin => match &state.global.stdio {
            StdioBackend::Passthrough => PolledStatus::ImmediatelyPollable,
            StdioBackend::Peered(_) => unreachable!(),
            StdioBackend::Feedback(feedback) => {
                PolledStatus::Pollable(feedback.read_polled.clone())
            }
            StdioBackend::Plugin(plugin_info) => {
                PolledStatus::Pollable(plugin_info.borrow().read_polled.clone())
            }
            StdioBackend::Sink => PolledStatus::NotPollable,
            StdioBackend::NullSink => PolledStatus::ImmediatelyPollable,
            StdioBackend::Fuzz(fuzz_endpoint) => {
                PolledStatus::Pollable(fuzz_endpoint.borrow().read_polled.clone())
            }
        },
        FdResource::Stdout => PolledStatus::ImmediatelyPollable,
        FdResource::Stderr => PolledStatus::ImmediatelyPollable,
        FdResource::Socket(socket_info) => match &socket_info.borrow().state {
            SocketState::Connectionless(connectionless) => match &connectionless.backend {
                ConnectionlessBackend::Passthrough => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::Peered(regular) => {
                    PolledStatus::Pollable(regular.read_polled.clone())
                }
                ConnectionlessBackend::Feedback(feedback) => {
                    PolledStatus::Pollable(feedback.read_polled.clone())
                }
                ConnectionlessBackend::Plugin(plugin_info) => {
                    PolledStatus::Pollable(plugin_info.borrow().read_polled.clone())
                }
                ConnectionlessBackend::Sink => PolledStatus::NotPollable,
                ConnectionlessBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::Fuzz(fuzz_endpoint) => {
                    PolledStatus::Pollable(fuzz_endpoint.borrow().read_polled.clone())
                }
            },
            SocketState::Unassociated(_) => PolledStatus::NotPollable,
            SocketState::Server(server) => PolledStatus::Pollable(server.ready_to_connect.clone()),
            SocketState::PendingConnection(_) => unreachable!(),
            SocketState::Connecting(_) => PolledStatus::NotPollable, // Need to select for writing, not reading
            SocketState::Connected(connected) => match &connected.backend {
                ConnectedBackend::Passthrough => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::Peered(regular) => {
                    PolledStatus::Pollable(regular.read_polled.clone())
                }
                ConnectedBackend::Feedback(feedback) => {
                    PolledStatus::Pollable(feedback.read_polled.clone())
                }
                ConnectedBackend::Plugin(plugin_info) => {
                    PolledStatus::Pollable(plugin_info.borrow().read_polled.clone())
                }
                ConnectedBackend::Sink => PolledStatus::NotPollable,
                ConnectedBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::Fuzz(fuzz_endpoint) => {
                    PolledStatus::Pollable(fuzz_endpoint.borrow().read_polled.clone())
                }
            },
        },
    }
}

pub fn fd_to_pollout(state: &mut FizzleState, fd: RawFd) -> PolledStatus {
    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(fd)) else {
        return PolledStatus::BadFd;
    };
    match &fd_info.resource {
        FdResource::Epoll(_) => panic!("polling an epoll descriptor not supported"),
        FdResource::EventFd(eventfd) => {
            PolledStatus::Pollable(eventfd.borrow().write_polled.clone())
        }
        FdResource::Directory(_) => PolledStatus::NotPollable,
        FdResource::File(_file_id) => PolledStatus::ImmediatelyPollable,
        /*
        match &state.global.files.get(&file_id).unwrap().backend {
            FileBackend::Passthrough | FileBackend::Peered(_) => unreachable!(),
            FileBackend::Feedback(feedback) => {
                PolledStatus::Pollable(feedback.write_polled.clone())
            }
            FileBackend::Plugin(plugin_id) => PolledStatus::Pollable(
                state
                    .global
                    .plugins
                    .get(&plugin_id)
                    .unwrap()
                    .write_polled
                    .clone(),
            ),
            FileBackend::Sink => PolledStatus::ImmediatelyPollable,
            FileBackend::NullSink => PolledStatus::ImmediatelyPollable,
            FileBackend::Fuzz(_) => PolledStatus::ImmediatelyPollable,
        },
        */
        FdResource::MessageQueue(_) => todo!(),
        FdResource::Pipe(pipe_info) => {
            if let Some(peer) = pipe_info.borrow().peer.upgrade() {
                PolledStatus::Pollable(peer.borrow().write_polled.clone())
            } else {
                PolledStatus::ImmediatelyPollable
            }
        }
        FdResource::Stdin => PolledStatus::NotPollable,
        FdResource::Stdout => match &state.global.stdio {
            StdioBackend::Passthrough => unreachable!(),
            StdioBackend::Peered(_) => unreachable!(),
            StdioBackend::Feedback(feedback) => {
                PolledStatus::Pollable(feedback.write_polled.clone())
            }
            StdioBackend::Plugin(plugin_info) => {
                PolledStatus::Pollable(plugin_info.borrow().write_polled.clone())
            }
            StdioBackend::Sink => PolledStatus::ImmediatelyPollable,
            StdioBackend::NullSink => PolledStatus::ImmediatelyPollable,
            StdioBackend::Fuzz(_) => PolledStatus::ImmediatelyPollable,
        },
        FdResource::Stderr => PolledStatus::NotPollable,
        FdResource::Socket(socket_info) => match &socket_info.borrow().state {
            SocketState::Connectionless(connectionless) => match &connectionless.backend {
                ConnectionlessBackend::Passthrough => unreachable!(),
                ConnectionlessBackend::Peered(_) => PolledStatus::ImmediatelyPollable, // A connectionless socket can always `send()` TODO: ??
                ConnectionlessBackend::Feedback(feedback) => {
                    PolledStatus::Pollable(feedback.write_polled.clone())
                }
                ConnectionlessBackend::Plugin(plugin_info) => {
                    PolledStatus::Pollable(plugin_info.borrow().write_polled.clone())
                }
                ConnectionlessBackend::Sink => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectionlessBackend::Fuzz(_) => PolledStatus::ImmediatelyPollable,
            },
            SocketState::Unassociated(_) => PolledStatus::NotPollable,
            SocketState::Server(_) => PolledStatus::NotPollable, // Need to select for reading, not writing
            SocketState::PendingConnection(_) => PolledStatus::NotPollable,
            SocketState::Connecting(connecting) => {
                PolledStatus::Pollable(connecting.connect_polled.clone())
            }
            SocketState::Connected(connected) => match &connected.backend {
                ConnectedBackend::Passthrough => unreachable!(),
                ConnectedBackend::Peered(peered) => {
                    if let Some(peer_info) = peered.peer.upgrade() {
                        let SocketState::Connected(conn) = &peer_info.borrow().state else {
                            unreachable!()
                        };

                        match &conn.backend {
                            ConnectedBackend::Peered(peer_info) => {
                                PolledStatus::Pollable(peer_info.write_polled.clone())
                            }
                            _ => panic!(),
                        }
                    } else {
                        PolledStatus::ImmediatelyPollable // The next `write()` call will return 0
                    }
                }
                ConnectedBackend::Feedback(feedback) => {
                    PolledStatus::Pollable(feedback.write_polled.clone())
                }
                ConnectedBackend::Plugin(plugin_info) => {
                    PolledStatus::Pollable(plugin_info.borrow().write_polled.clone())
                }
                ConnectedBackend::Sink => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::NullSink => PolledStatus::ImmediatelyPollable,
                ConnectedBackend::Fuzz(_) => PolledStatus::ImmediatelyPollable,
            },
            // SocketState::Error => PolledStatus::ImmediatelyPollable,
        },
    }
}
