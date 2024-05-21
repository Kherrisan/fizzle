use super::{DirectoryId, FileId, MessageQueueId, PipeId, SocketId};

#[derive(Debug)]
pub struct FdInfo {
    /// Whether the file descriptor associated with closes on calls to `exec()`.
    pub close_on_exec: bool,
    /// Whether the descriptor is configured to block on input or not.
    pub nonblocking: bool,
    /// The resource the file descriptor points to.
    pub resource: FdResource,
}

#[derive(Debug)]
pub enum FdResource {
    /// Files `open()`ed using O_PATH
    Directory(DirectoryId),
    /// Files that are accessed via the virtual filesystem.
    File(FileId),
    /// Cross-process message queues.
    #[allow(unused)]
    MessageQueue(MessageQueueId),
    /// Files that are accessed normally.
    PassthroughFile,
    /// Anonymous pipes, such as those created with `pipe()`.
    Pipe(PipeId),
    /// Network sockets.
    #[allow(unused)]
    Socket(SocketId),
}
