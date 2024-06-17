use std::{mem::MaybeUninit, os::fd::RawFd, thread::ThreadId};

use fizzle_common::{
    io::MAX_PATH_LEN, path::FilePath, storage::{ArenaKey, Buffer}
};
use fizzle_plugin::FizzlePluginObject;

use crate::{constants::*, semaphore::Semaphore};

use super::{
    backend::FileBackend, fd::FdInfo, EpollInfo, EventFdInfo,
    MessageQueueInfo, PipeInfo, PluginInfo, PolledInfo, PollerInfo, SemaphoreInfo, SocketState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId(usize);

impl ArenaKey for BufferId {
    type Value = Buffer<FIZZLE_BUFFER_LENGTH>;
}

impl From<usize> for BufferId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<BufferId> for usize {
    fn from(value: BufferId) -> Self {
        value.0
    }
}

/// An identifier used to represent a valid file descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DescriptorId(usize);

impl DescriptorId {
    pub fn new(fd: RawFd) -> Self {
        Self(fd as usize)
    }
}

impl ArenaKey for DescriptorId {
    type Value = FdInfo;
}

impl From<usize> for DescriptorId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<DescriptorId> for usize {
    fn from(val: DescriptorId) -> Self {
        val.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DirectoryId(usize);

impl ArenaKey for DirectoryId {
    type Value = FilePath<MAX_PATH_LEN>;
}

impl From<usize> for DirectoryId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<DirectoryId> for usize {
    fn from(val: DirectoryId) -> Self {
        val.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EpollId(usize);

impl ArenaKey for EpollId {
    type Value = EpollInfo;
}

impl From<usize> for EpollId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<EpollId> for usize {
    fn from(val: EpollId) -> Self {
        val.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventFdId(usize);

impl ArenaKey for EventFdId {
    type Value = EventFdInfo;
}

impl From<usize> for EventFdId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<EventFdId> for usize {
    fn from(val: EventFdId) -> Self {
        val.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    identifier: usize,
}

impl ArenaKey for FileId {
    type Value = FileBackend;
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

impl ArenaKey for MessageQueueId {
    type Value = MessageQueueInfo;
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

impl ArenaKey for PipeId {
    type Value = PipeInfo;
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
pub struct PluginModuleId(usize);

impl ArenaKey for PluginModuleId {
    type Value = Box<dyn FizzlePluginObject>;
}

impl From<usize> for PluginModuleId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<PluginModuleId> for usize {
    fn from(value: PluginModuleId) -> usize {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginId(usize);

impl ArenaKey for PluginId {
    type Value = PluginInfo;
}

impl From<usize> for PluginId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<PluginId> for usize {
    fn from(value: PluginId) -> usize {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PolledId(usize);

impl ArenaKey for PolledId {
    type Value = PolledInfo;
}

impl From<usize> for PolledId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<PolledId> for usize {
    fn from(value: PolledId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PollerId(usize);

impl ArenaKey for PollerId {
    type Value = PollerInfo;
}

impl From<usize> for PollerId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<PollerId> for usize {
    fn from(value: PollerId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId(usize);

impl ArenaKey for ProcessId {
    type Value = MaybeUninit<Semaphore>;
}

impl From<usize> for ProcessId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<ProcessId> for usize {
    fn from(val: ProcessId) -> Self {
        val.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemaphoreId {
    identifier: usize,
}

impl ArenaKey for SemaphoreId {
    type Value = SemaphoreInfo;
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

impl ArenaKey for SocketId {
    type Value = SocketState;
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
    pub process_id: ProcessId,
    pub thread_id: ThreadId,
}

// ==============================================
//           Pointer-Based Identifiers
// ==============================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BarrierPtr(usize);

impl From<*mut libc::pthread_barrier_t> for BarrierPtr {
    fn from(value: *mut libc::pthread_barrier_t) -> Self {
        BarrierPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CondVarPtr(usize);

impl From<*mut libc::pthread_cond_t> for CondVarPtr {
    fn from(value: *mut libc::pthread_cond_t) -> Self {
        CondVarPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MutexPtr(usize);

impl From<*mut libc::pthread_mutex_t> for MutexPtr {
    fn from(value: *mut libc::pthread_mutex_t) -> Self {
        MutexPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RwLockPtr(usize);

impl From<*mut libc::pthread_rwlock_t> for RwLockPtr {
    fn from(value: *mut libc::pthread_rwlock_t) -> Self {
        RwLockPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpinlockPtr(usize);

impl From<*mut libc::pthread_spinlock_t> for SpinlockPtr {
    fn from(value: *mut libc::pthread_spinlock_t) -> Self {
        SpinlockPtr(value as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SemaphorePtr(usize);

impl From<*mut libc::sem_t> for SemaphorePtr {
    fn from(value: *mut libc::sem_t) -> Self {
        SemaphorePtr(value as usize)
    }
}

impl SemaphorePtr {
    pub(crate) fn to_mut_ptr(self) -> *mut libc::sem_t {
        self.0 as *mut libc::sem_t
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePtr(usize);

impl From<*mut libc::FILE> for FilePtr {
    fn from(value: *mut libc::FILE) -> Self {
        FilePtr(value as usize)
    }
}
