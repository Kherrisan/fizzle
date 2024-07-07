use crate::arena::{ArenaKey, Rc}; 
use super::directory::DirectoryId;
use super::epoll::EpollId;
use super::eventfd::EventfdId;
use super::file::FileId;
use super::message_queue::MessageQueueId;
use super::pipe::PipeId;
use super::socket::SocketId;

pub use private::DescriptorId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    use std::os::fd::RawFd;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct DescriptorId(usize);

    impl DescriptorId {
        pub fn from_raw_fd(fd: RawFd) -> Self {
            DescriptorId(fd as usize)
        }
    }
}

#[derive(Clone, Debug)]
pub struct DescriptorInfo {
    /// Whether the file descriptor associated with closes on calls to `exec()`.
    pub close_on_exec: bool,
    /// Whether the descriptor is configured to block on input or not.
    pub nonblocking: bool,
    pub is_passthrough: bool,
    /// The resource the file descriptor points to.
    pub resource: FdResource,
}

impl DescriptorInfo {
    pub fn new(resource: FdResource) -> Self {
        Self {
            close_on_exec: false,
            nonblocking: false,
            is_passthrough: false,
            resource,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FdResource {
    /// Files `open()`ed using O_PATH
    Directory(Rc<DirectoryId>),
    /// Epoll descriptors.
    Epoll(Rc<EpollId>),
    /// Event file descriptor.
    EventFd(Rc<EventfdId>),
    /// Files that are accessed via the virtual filesystem.
    File(Rc<FileId>),
    /// Cross-process message queues.
    #[allow(unused)]
    MessageQueue(Rc<MessageQueueId>),
    /// Anonymous pipes, such as those created with `pipe()`.
    Pipe(Rc<PipeId>),
    /// The standard input of the parent process (which may be inherited by children).
    Stdin,
    /// The standard output of the parent process. (which may be inherited by children).
    Stdout,
    /// The standard error of the parent process. (which may be inherited by children).
    Stderr,
    /// Network sockets.
    Socket(Rc<SocketId>),
}

impl ArenaKey for DescriptorId {
    type Value = DescriptorInfo;
}

impl DescriptorId {

}
