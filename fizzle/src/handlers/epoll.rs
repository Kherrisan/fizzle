use super::descriptor::{Descriptor, ReadData, WriteData};
use super::polled::PolledInfo;
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;
use crate::{GlobalMap, GlobalRc};

pub struct EpollInfo {
    pub interests: GlobalMap<Descriptor, EpollInterest>,
}

#[derive(Clone)]
pub struct EpollInterest {
    pub direction: EpollDirection,
    pub user_data: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum EpollDirection {
    None,
    Read(PolledStatus),
    Write(PolledStatus),
    Both(PolledStatus, PolledStatus),
}

#[derive(Clone, PartialEq, Eq)]
pub enum PolledStatus {
    Pollable(GlobalRc<PolledInfo>),
    /// The file descriptor was invalid.
    BadFd,
    /// The requested object will never return polled output (such as attempting to read `stdout`).
    NotPollable,
    /// The requested object will immediately return polled output (such as writing to `stderr`).
    ImmediatelyPollable,
}

pub struct EpollReadEvent<'a> {
    fd: Descriptor,
    data: ReadData<'a>,
}

impl<'a> EpollReadEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: ReadData<'a>) -> Self {
        Self { fd, data }
    }
}

impl Event for EpollReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        log::error!("read() called directly on epoll socket");
        Outcome::Error(Errno::EBADF)
    }
}

pub struct EpollWriteEvent<'a> {
    fd: Descriptor,
    data: WriteData<'a>,
}

impl<'a> EpollWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self { fd, data }
    }
}

impl Event for EpollWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        log::error!("write() called directly on epoll socket");
        Outcome::Error(Errno::EBADF)
    }
}
