use fizzle_common::storage::Rc;

use crate::state::identifiers::*;

#[derive(Clone, Debug)]
pub struct FdInfo {
    /// Whether the file descriptor associated with closes on calls to `exec()`.
    pub close_on_exec: bool,
    /// Whether the descriptor is configured to block on input or not.
    pub nonblocking: bool,
    pub is_passthrough: bool,
    /// The resource the file descriptor points to.
    pub resource: FdResource,
}

impl FdInfo {
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
    EventFd(Rc<EventFdId>),
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
