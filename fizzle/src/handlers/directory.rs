use crate::arena::ArenaKey;
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;

pub use private::DirectoryId;

use super::descriptor::{Descriptor, ReadData, WriteData};

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
    #[repr(transparent)]
    pub struct DirectoryId(usize);
}

impl ArenaKey for DirectoryId {
    type Value = FilePath<MAX_PATH_LEN>;
}

pub struct DirectoryReadEvent<'a> {
    fd: Descriptor,
    data: ReadData<'a>,
}

impl<'a> DirectoryReadEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: ReadData<'a>) -> Self {
        Self { fd, data }
    }
}

impl Event for DirectoryReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        unimplemented!("read() operation unsupported for directories")
    }
}

pub struct DirectoryWriteEvent<'a> {
    fd: Descriptor,
    data: WriteData<'a>,
}

impl<'a> DirectoryWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self { fd, data }
    }
}

impl Event for DirectoryWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        unimplemented!("write() operation unsupported for directories")
    }
}
