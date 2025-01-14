use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use super::descriptor::{Descriptor, ReadData, WriteData};

pub struct DirectoryInfo {
    pub path: FilePath<MAX_PATH_LEN>,
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
