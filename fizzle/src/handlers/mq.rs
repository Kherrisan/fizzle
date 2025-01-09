use crate::errno::Errno;
use crate::scheduler::Event;
use crate::state::FizzleState;
use crate::{arena::ArenaKey, scheduler::Outcome};

pub use private::MqId;

use super::descriptor::{Descriptor, ReadData, WriteData};

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct MqId(usize);
}

#[derive(Debug)]
pub struct MqInfo {}

impl ArenaKey for MqId {
    type Value = MqInfo;
}

pub struct MqReadEvent<'a> {
    fd: Descriptor,
    data: ReadData<'a>,
}

impl<'a> MqReadEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: ReadData<'a>) -> Self {
        Self { fd, data }
    }
}

impl Event for MqReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        todo!()
    }
}

pub struct MqWriteEvent<'a> {
    fd: Descriptor,
    data: WriteData<'a>,
}

impl<'a> MqWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self { fd, data }
    }
}

impl Event for MqWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        todo!()
    }
}
