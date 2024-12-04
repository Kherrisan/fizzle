use super::descriptor::{DescriptorId, ReadData, WriteData};
use super::polled::PolledId;
use crate::arena::{ArenaKey, Rc};
use crate::constants::FIZZLE_MAX_EPOLL_FDS;
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use heapless::FnvIndexMap;

pub use private::EpollId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct EpollId(usize);
}

#[derive(Debug)]
pub struct EpollInfo {
    pub interests: FnvIndexMap<DescriptorId, EpollInterest, FIZZLE_MAX_EPOLL_FDS>,
}

#[derive(Clone, Debug)]
pub struct EpollInterest {
    pub direction: EpollDirection,
    pub user_data: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpollDirection {
    None,
    Read(PolledStatus),
    Write(PolledStatus),
    Both(PolledStatus, PolledStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolledStatus {
    Pollable(Rc<PolledId>),
    /// The file descriptor was invalid.
    BadFd,
    /// The requested object will never return polled output (such as attempting to read `stdout`).
    NotPollable,
    /// The requested object will immediately return polled output (such as writing to `stderr`).
    ImmediatelyPollable,
}

impl ArenaKey for EpollId {
    type Value = EpollInfo;
}

pub struct EpollReadEvent<'a> {
    fd: DescriptorId,
    data: ReadData<'a>,
}

impl<'a> EpollReadEvent<'a> {
    #[inline]
    pub fn new(fd: DescriptorId, data: ReadData<'a>) -> Self {
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
    fd: DescriptorId,
    data: WriteData<'a>,
}

impl<'a> EpollWriteEvent<'a> {
    #[inline]
    pub fn new(fd: DescriptorId, data: WriteData<'a>) -> Self {
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
