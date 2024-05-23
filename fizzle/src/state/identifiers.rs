use std::{os::fd::RawFd, thread::ThreadId};


#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId {
    identifier: usize,
}

impl BufferId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for BufferId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<BufferId> for usize {
    fn from(val: BufferId) -> Self {
        val.identifier
    }
}

/// An identifier used to represent a valid file descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DescriptorId {
    identifier: usize,
}

impl DescriptorId {
    pub fn new(fd: RawFd) -> Self {
        Self {
            identifier: fd as usize,
        }
    }
}

impl From<usize> for DescriptorId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<DescriptorId> for usize {
    fn from(val: DescriptorId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryId {
    identifier: usize,
}

impl DirectoryId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for DirectoryId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<DirectoryId> for usize {
    fn from(val: DirectoryId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FifoId {
    identifier: usize,
}

impl FifoId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<FifoId> for usize {
    fn from(val: FifoId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    identifier: usize,
}

impl FileId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for FileId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<FileId> for usize {
    fn from(val: FileId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MessageQueueId {
    identifier: usize,
}

impl MessageQueueId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for MessageQueueId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<MessageQueueId> for usize {
    fn from(val: MessageQueueId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipeId {
    identifier: usize,
}

impl PipeId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for PipeId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<PipeId> for usize {
    fn from(val: PipeId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId {
    identifier: usize,
}

impl ProcessId {
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<ProcessId> for usize {
    fn from(val: ProcessId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemaphoreId {
    identifier: usize,
}

impl SemaphoreId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for SemaphoreId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<SemaphoreId> for usize {
    fn from(val: SemaphoreId) -> Self {
        val.identifier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SocketId {
    identifier: usize,
}

impl SocketId {
    #[allow(unused)]
    pub fn new(ident: usize) -> Self {
        Self { identifier: ident }
    }
}

impl From<usize> for SocketId {
    fn from(value: usize) -> Self {
        Self { identifier: value }
    }
}

impl From<SocketId> for usize {
    fn from(val: SocketId) -> Self {
        val.identifier
    }
}

/// The unique identifying information for a given thread in a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerId {
    pub process: ProcessId,
    pub thread: ThreadId,
}
