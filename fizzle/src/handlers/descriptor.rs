use std::cmp;
use std::os::fd::RawFd;

use super::directory::DirectoryId;
use super::epoll::EpollId;
use super::eventfd::EventfdId;
use super::file::FileId;
use super::fuzz_endpoint::FuzzEndpointInfo;
use super::message_queue::MessageQueueId;
use super::pipe::PipeId;
use super::socket::{LocalAddress, SocketId, SocketState};
use super::{init_from_slice, FfiOutput, MsgFlags, MsgHdr, MsgHdrOut};
use crate::arena::{ArenaKey, Rc};
use crate::backend::{ConnectedBackend, StdioBackend};
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::{FizzleSingleton, FizzleState};

use fizzle_common::io::TransportAddress;
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

        pub fn as_raw_fd(&self) -> RawFd {
            self.0 as RawFd
        }
    }
}

#[derive(Clone, Debug)]
pub struct DescriptorInfo {
    /// Whether the file descriptor associated with closes on calls to `exec()`.
    pub close_on_exec: bool,
    /// Whether the descriptor is configured to block on input or not.
    pub nonblocking: bool,
    /// Whether the file descriptor represents a real file descriptor or an emulated one.
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
    pub fn write(
        &self,
        ctx: &mut FizzleSingleton,
        msg: &impl MsgHdr,
    ) -> Result<usize, DescriptorError> {
        let state = ctx.acquire();

        let Some(fd_info) = state.local.fds.get(self) else {
            return Err(DescriptorError::BadFd);
        };

        let nonblocking = fd_info.nonblocking || msg.flags().contains(MsgFlags::DONTWAIT);
        let resource = fd_info.resource.clone();
        drop(state);

        match resource {
            FdResource::Directory(_) => unimplemented!(),
            FdResource::Epoll(_) => unimplemented!(),
            FdResource::EventFd(eventfd_id) => eventfd_id
                .write(ctx, msg, nonblocking)
                .map_err(|e| e.into()),
            FdResource::File(file_id) => file_id.write(ctx, msg).map_err(|e| e.into()),
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => pipe_id.write(ctx, msg, nonblocking).map_err(|e| e.into()),
            FdResource::Socket(socket_id) => {
                socket_id.write(ctx, msg, nonblocking).map_err(|e| e.into())
            }
            FdResource::Stdin | FdResource::Stdout => {
                let state = ctx.acquire();
                // Writing to `stdin` is equivalent to writing to `stdout` in most scenarios

                let total_len = msg.vdata().iter().map(|v| v.data().len()).sum();

                match &state.global.stdio {
                    StdioBackend::Passthrough | StdioBackend::Peered(_) => unreachable!(),
                    StdioBackend::Feedback(feedback) => {
                        let buffer_id = feedback.buf.clone();
                        let write_polled = feedback.write_polled.clone();
                        let read_polled = feedback.read_polled.clone();

                        let event_raised = state
                            .global
                            .polled_events
                            .get(&write_polled)
                            .unwrap()
                            .event_raised;
                        drop(state);

                        if !event_raised {
                            if nonblocking {
                                return Err(DescriptorError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.buffers.get_mut(&buffer_id).unwrap();
                        let mut total_written = 0;
                        for iovec in msg.vdata() {
                            if buf.is_full() {
                                break;
                            }
                            total_written += buf.write(iovec.data());
                        }

                        if buf.is_full() {
                            state.lower_polled(&write_polled);
                        }
                        state.raise_polled(&read_polled);

                        Ok(total_written)
                    }
                    StdioBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                        let buffer_id = plugin_info.write_buf.clone();
                        let write_polled = plugin_info.write_polled.clone();

                        let event_raised = state
                            .global
                            .polled_events
                            .get(&write_polled)
                            .unwrap()
                            .event_raised;
                        drop(state);

                        if !event_raised {
                            if nonblocking {
                                return Err(DescriptorError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(write_polled.clone());
                            }
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.buffers.get_mut(&buffer_id).unwrap();
                        let mut total_written = 0;
                        for iovec in msg.vdata() {
                            if buf.is_full() {
                                break;
                            }
                            total_written += buf.write(iovec.data());
                        }

                        Ok(total_written)
                    }
                    StdioBackend::Sink => Ok(total_len),
                    StdioBackend::NullSink => Ok(total_len),
                    StdioBackend::Fuzz(_) => Ok(total_len),
                }
            }
            FdResource::Stderr => {
                let res = unsafe {
                    libc::writev(
                        2,
                        msg.vdata().as_ptr() as *const libc::iovec,
                        msg.vdata().len() as i32,
                    )
                };
                match res {
                    0.. => Ok(res as usize),
                    _ => Err(DescriptorError::Passthrough),
                }
            }
        }
    }

    pub fn read(
        &self,
        ctx: &mut FizzleSingleton,
        msg: &mut MsgHdrOut,
    ) -> Result<usize, DescriptorError> {
        let state = ctx.acquire();

        let Some(fd_info) = state.local.fds.get(self) else {
            return Err(DescriptorError::BadFd);
        };

        let nonblocking = fd_info.nonblocking || msg.flags_mut().contains(MsgFlags::DONTWAIT);
        let resource = fd_info.resource.clone();
        drop(state);

        match resource {
            FdResource::Directory(_) => unimplemented!(),
            FdResource::Epoll(_) => unimplemented!(),
            FdResource::EventFd(eventfd_id) => {
                eventfd_id.read(ctx, msg, nonblocking).map_err(|e| e.into())
            }
            FdResource::File(file_id) => file_id.read(ctx, msg).map_err(|e| e.into()),
            FdResource::MessageQueue(_) => todo!(),
            FdResource::Pipe(pipe_id) => pipe_id.read(ctx, msg, nonblocking).map_err(|e| e.into()),
            FdResource::Socket(socket_id) => {
                socket_id.read(ctx, msg, nonblocking).map_err(|e| e.into())
            }
            FdResource::Stdin | FdResource::Stdout | FdResource::Stderr => {
                let mut state = ctx.acquire();

                match &state.global.stdio {
                    StdioBackend::Passthrough | StdioBackend::Peered(_) => unreachable!(),
                    StdioBackend::Feedback(feedback) => {
                        let buffer_id = feedback.buf.clone();
                        let read_polled = feedback.read_polled.clone();
                        let write_polled = feedback.write_polled.clone();
                        let event_raised = state
                            .global
                            .polled_events
                            .get(&read_polled)
                            .unwrap()
                            .event_raised;

                        drop(state);

                        if !event_raised {
                            if nonblocking {
                                return Err(DescriptorError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(read_polled.clone());
                            }
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.buffers.get_mut(&buffer_id).unwrap();
                        let mut total_read = 0;

                        for iovec in msg.vdata_mut() {
                            if buf.is_empty() {
                                break;
                            }

                            let data_len = cmp::min(buf.len(), iovec.data_mut().len());
                            init_from_slice(
                                &mut iovec.data_mut()[..data_len],
                                &buf.data()[..data_len],
                            );
                            buf.did_read(data_len);
                            total_read += data_len;
                        }

                        if buf.is_empty() {
                            state.lower_polled(&read_polled);
                        }
                        state.raise_polled(&write_polled);

                        Ok(total_read)
                    }
                    StdioBackend::Plugin(plugin_id) => {
                        let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                        let buffer_id = plugin_info.write_buf.clone();
                        let read_polled = plugin_info.read_polled.clone();
                        let event_raised = state
                            .global
                            .polled_events
                            .get(&read_polled)
                            .unwrap()
                            .event_raised;

                        drop(state);

                        if !event_raised {
                            if nonblocking {
                                return Err(DescriptorError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(read_polled.clone());
                            }
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.buffers.get_mut(&buffer_id).unwrap();
                        let mut total_read = 0;

                        for iovec in msg.vdata_mut() {
                            if buf.is_empty() {
                                break;
                            }

                            let data_len = cmp::min(buf.len(), iovec.data_mut().len());
                            init_from_slice(
                                &mut iovec.data_mut()[..data_len],
                                &buf.data()[..data_len],
                            );
                            buf.did_read(data_len);
                            total_read += data_len;
                        }

                        if buf.is_empty() {
                            state.lower_polled(&read_polled);
                        }

                        Ok(total_read)
                    }
                    StdioBackend::Sink => Ok(0),
                    StdioBackend::NullSink => {
                        let mut total_read = 0;
                        for iovec in msg.vdata_mut() {
                            for b in iovec.data_mut() {
                                b.write(0);
                            }
                            total_read += iovec.data_mut().len();
                        }

                        Ok(total_read)
                    }
                    StdioBackend::Fuzz(fuzz_endpoint_id) => {
                        let fuzz_endpoint_id = fuzz_endpoint_id.clone();
                        let FuzzEndpointInfo {
                            mut read_idx,
                            read_polled,
                        } = state
                            .global
                            .fuzz_endpoints
                            .get(&fuzz_endpoint_id)
                            .unwrap()
                            .clone();

                        let polled_is_ready = state.polled_is_ready(&read_polled);
                        drop(state);

                        if !polled_is_ready {
                            if nonblocking {
                                return Err(DescriptorError::WouldBlock);
                            } else {
                                ctx.poll_until_ready(read_polled.clone());
                            }
                        }

                        let mut state = ctx.acquire();

                        let buf = state.global.fuzz_input.data();
                        let buflen = buf.len();

                        let mut total_read = 0;
                        for iovec in msg.vdata_mut() {
                            if buf[read_idx..].is_empty() {
                                break;
                            }

                            let data_len = cmp::min(buf.len(), iovec.data_mut().len());
                            init_from_slice(
                                &mut iovec.data_mut()[..data_len],
                                &buf[read_idx..read_idx + data_len],
                            );
                            read_idx += data_len;
                            total_read += data_len;
                        }

                        let fuzz_endpoint = state
                            .global
                            .fuzz_endpoints
                            .get_mut(&fuzz_endpoint_id)
                            .unwrap();
                        fuzz_endpoint.read_idx = read_idx;
                        if fuzz_endpoint.read_idx == buflen {
                            state.lower_polled(&read_polled);
                        }

                        Ok(total_read)
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum DescriptorError {
    /// A non-blocking operation on the given descriptor would cause it to block.
    WouldBlock,
    /// The given file descriptor was not found in the Fizzle state.
    BadFd,
    /// The given file descriptor was not a socket.
    NotSocket,
    /// The operation passed to the given descriptor had invalid inputs.
    InvalidInput,
    /// A descriptor did not have an active connection.
    NotConnected,
    /// A descriptor already had an active connection.
    IsConnected,
    /// A supplied address was already bound to.
    AddressInUse,
    /// An attempted connection failed due to the endpoint not listening.
    ConnectionRefused,
    /// An initiated connection has not yet completed.
    ConnectInProgress,
    /// The write end of a pipe has closed.
    PipeClosed,
    /// An error supplied by a libc call.
    Passthrough,
}

impl FfiOutput for Result<usize, DescriptorError> {
    type OutputType = libc::ssize_t;

    fn out(&self) -> Self::OutputType {
        match self {
            Ok(i) => {
                Self::set_errno(0);
                return *i as libc::ssize_t;
            }
            Err(DescriptorError::WouldBlock) => Self::set_errno(libc::EAGAIN),
            Err(DescriptorError::BadFd) => Self::set_errno(libc::EBADFD),
            Err(DescriptorError::NotSocket) => Self::set_errno(libc::ENOTSOCK),
            Err(DescriptorError::InvalidInput) => Self::set_errno(libc::EINVAL),
            Err(DescriptorError::NotConnected) => Self::set_errno(libc::ENOTCONN),
            Err(DescriptorError::IsConnected) => Self::set_errno(libc::EISCONN),
            Err(DescriptorError::AddressInUse) => Self::set_errno(libc::EADDRINUSE),
            Err(DescriptorError::ConnectionRefused) => Self::set_errno(libc::ECONNREFUSED),
            Err(DescriptorError::ConnectInProgress) => Self::set_errno(libc::EINPROGRESS),
            Err(DescriptorError::PipeClosed) => Self::set_errno(libc::EPIPE),
            Err(DescriptorError::Passthrough) => (),
        }

        -1
    }

    fn display(&self) -> &'static str {
        match self {
            Ok(0) => "0",
            Ok(_) => ">0",
            Err(DescriptorError::WouldBlock) => "-1 (EAGAIN)",
            Err(DescriptorError::BadFd) => "-1 (EBADFD)",
            Err(DescriptorError::NotSocket) => "-1 (ENOTSOCK)",
            Err(DescriptorError::InvalidInput) => "-1 (EINVAL)",
            Err(DescriptorError::NotConnected) => "-1 (ENOTCONN)",
            Err(DescriptorError::IsConnected) => "-1 (EISCONN)",
            Err(DescriptorError::AddressInUse) => "-1 (EADDRINUSE)",
            Err(DescriptorError::ConnectionRefused) => "-1 (ECONNREFUSED)",
            Err(DescriptorError::ConnectInProgress) => "-1 (EINPROGRESS)",
            Err(DescriptorError::PipeClosed) => "-1 (EPIPE)",
            Err(DescriptorError::Passthrough) => "-1 (passthrough error)",
        }
    }
}

pub struct DescriptorCloseEvent {
    fd: DescriptorId,
}

impl DescriptorCloseEvent {
    #[inline]
    pub fn new(fd: DescriptorId) -> Self {
        Self { fd }
    }
}

impl Event for DescriptorCloseEvent {
    type Success = ();
    type Error = Errno;

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        let Some(fd_info) = state.local.fds.get(&self.fd) else {
            return Outcome::Error(Errno::EBADFD)
        };

        if let FdResource::Socket(socket_id) = fd_info.resource.clone() {
            // Decrement the number of fd references to the socket
            let socket_info = state.global.sockets.get_mut(&socket_id).unwrap();
            socket_info.fd_count.checked_sub(1).unwrap();

            // Is this the last file descriptor referencing the socket?
            if socket_info.fd_count == 0 {
                // Remove the socket's address from the global space
                if let LocalAddress::Assigned(sockaddr) = socket_info.local_addr.clone() {
                    let protocol = socket_info.protocol;
                    state.global.socket_locations.remove(&TransportAddress {
                        sockaddr,
                        protocol,
                    }).unwrap();
                }

                // Certain socket states contain cyclic references with other sockets.
                // We need to manually remove these to
                if let SocketState::Connected(connected) = &mut socket_info.state {
                    if !connected.peer_closed {
                        connected.peer_closed = true;
                        if let ConnectedBackend::Peered(peer_info) = &mut connected.backend {
                            // TODO: do we take the peer's socket ID here so that we can set peer_closed = true on it?
                            // TODO: do we raise the poll of the peer here?
                            peer_info.peer = None;
                        }
                    }
                }
            }
        }

        // Destroy the underlying file descriptor in use.
        match &fd_info.resource {
            FdResource::Stdin | FdResource::Stdout | FdResource::Stderr => (),
            _ => crate::destroy_descriptor(self.fd.as_raw_fd()),
        };

        // fds never have more than one reference at a time, so this will always destroy a fd.
        state.local.fds.downref(&self.fd);

        Outcome::Success(())
    }
}

pub struct DescriptorDuplicateEvent {
    old_fd: DescriptorId,
    new_fd: Option<DescriptorId>,
    close_on_exec: bool,
}

impl DescriptorDuplicateEvent {
    #[inline]
    pub fn new(old_fd: DescriptorId, new_fd: Option<DescriptorId>, close_on_exec: bool) -> Self {
        Self { old_fd, new_fd, close_on_exec }
    }
}

impl Event for DescriptorDuplicateEvent {
    type Success = RawFd;
    type Error = Errno;

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        // Copy over associated data from the old fd
        let Some(mut new_fd_info) = state.local.fds.get_mut(&self.old_fd).cloned() else {
            return Outcome::Error(Errno::EBADFD)
        };


        // Create a new, unique file descriptor
        let new_fd = match self.new_fd {
            Some(fd) => fd,
            None => DescriptorId::from_raw_fd(crate::create_descriptor()),
        };

        if self.old_fd == new_fd {
            return Outcome::Error(Errno::EINVAL) // Behavior for dup3 (dup2 is different)
        }

        // Update the close-on-exec flag
        new_fd_info.close_on_exec = self.close_on_exec;

        // Upref the file descriptor count where applicable
        if let FdResource::Socket(socket_id) = new_fd_info.resource.clone() {
            state.global.sockets.get_mut(&socket_id).unwrap().fd_count += 1;
        }

        // Close `newfd` if it points to an occupied descriptor
        if state.local.fds.is_occupied(&new_fd) {
            // TODO: this is dangerous(ish), as the behavior of the run event loop may change.
            // Figure out how to call events within events more ergonomically while not exposing
            // `ctx`
            match DescriptorCloseEvent::new(new_fd).run(state) {
                Outcome::Success(()) => (),
                Outcome::Error(_) => unreachable!("internal state inconsistency"),
                _ => unreachable!(),
            }
        }

        state.local.fds.allocate_with_key(new_fd, new_fd_info).unwrap();

        Outcome::Success(new_fd.as_raw_fd())
    }
}
