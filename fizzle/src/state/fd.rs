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

#[derive(Clone, Copy, Debug)]
pub enum FdResource {
    /// Files `open()`ed using O_PATH
    Directory(DirectoryId),
    /// Epoll descriptors.
    Epoll(EpollId),
    /// Files that are accessed via the virtual filesystem.
    File(FileId),
    /// Cross-process message queues.
    #[allow(unused)]
    MessageQueue(MessageQueueId),
    /// Anonymous pipes, such as those created with `pipe()`.
    Pipe(PipeId),
    /// The standard input of the parent process (which may be inherited by children).
    Stdin,
    /// The standard output of the parent process. (which may be inherited by children).
    Stdout,
    /// The standard error of the parent process. (which may be inherited by children).
    Stderr,
    /// Network sockets.
    #[allow(unused)]
    Socket(SocketId),
}
