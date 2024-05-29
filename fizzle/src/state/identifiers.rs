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
pub struct DescriptorId(usize);

impl DescriptorId {
    pub fn new(fd: RawFd) -> Self {
        Self(fd as usize)
    }
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
pub struct FifoId(usize);

impl From<FifoId> for usize {
    fn from(val: FifoId) -> Self {
        val.0
    }
}

impl From<usize> for FifoId {
    fn from(val: usize) -> Self {
        Self(val)
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

impl From<usize> for PluginModuleId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Into<usize> for PluginModuleId {
    fn into(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginId(usize);

impl PluginId {
    pub const INVALID: PluginId = PluginId(usize::MAX);
}

impl From<usize> for PluginId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Into<usize> for PluginId {
    fn into(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PolledId(usize);

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
