use std::{cmp, ptr};
use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::rc::Weak;

use super::directory::*;
use super::epoll::*;
use super::eventfd::*;
use super::file::*;
use super::fuzz_endpoint::FuzzEndpointInfo;
use super::inotify::InotifyInfo;
use super::mq::*;
use super::pipe::*;
use super::poller::PollerInfo;
use super::socket::*;
use crate::backend::{ConnectedBackend, StdioBackend};
use crate::errno::Errno;
use crate::scheduler::{fizzle_alloc, Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::GlobalRc;

use bitflags::bitflags;
use fizzle_common::io::TransportAddress;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Descriptor(usize);

impl Descriptor {
    pub fn from_raw_fd(fd: RawFd) -> Self {
        Descriptor(fd as usize)
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.0 as RawFd
    }
}

#[derive(Clone)]
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

#[derive(Clone)]
pub enum FdResource {
    /// Files `open()`ed using O_PATH
    Directory(GlobalRc<DirectoryInfo>),
    /// Epoll descriptors.
    Epoll(GlobalRc<EpollInfo>),
    /// Event file descriptor.
    EventFd(GlobalRc<EventfdInfo>),
    /// Files that are accessed via the virtual filesystem.
    File(GlobalRc<OpenFileInfo>),
    Inotify(GlobalRc<InotifyInfo>),
    /// Cross-process message queues.
    #[allow(unused)]
    MessageQueue(GlobalRc<MqId>),
    /// Anonymous pipes, such as those created with `pipe()`.
    Pipe(GlobalRc<PipeInfo>),
    /// The standard input of the parent process (which may be inherited by children).
    Stdin,
    /// The standard output of the parent process. (which may be inherited by children).
    Stdout,
    /// The standard error of the parent process. (which may be inherited by children).
    Stderr,
    /// Network sockets.
    Socket(GlobalRc<SocketInfo>),
    /// An opaque fd meant only to be used in passthrough
    Opaque,
}

pub struct DescriptorCloseEvent {
    fd: Descriptor,
}

impl DescriptorCloseEvent {
    #[inline]
    pub fn new(fd: Descriptor) -> Self {
        Self { fd }
    }
}

impl Event for DescriptorCloseEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.fd) else {
            // #[cfg(not(feature = "passthroughfs"))]
            return Outcome::Error(Errno::EBADF);
            /*
            #[cfg(feature = "passthroughfs")]
            return match unsafe { libc::close(self.fd.as_raw_fd()) } {
                0 => Outcome::Success(()),
                _ => Outcome::Error(Errno::get_errno()),
            };
            */
        };

        if let FdResource::Socket(socket_info) = fd_info.resource.clone() {
            // Decrement the number of fd references to the socket
            socket_info.borrow_mut().fd_count.checked_sub(1).unwrap();

            // Is this the last file descriptor referencing the socket?
            if socket_info.borrow().fd_count == 0 {
                // Remove the socket's address from the global space
                if let LocalAddress::Assigned(sockaddr) = socket_info.borrow().local_addr.clone() {
                    let protocol = socket_info.borrow().protocol;
                    state
                        .global
                        .socket_locations
                        .remove(&TransportAddress { sockaddr, protocol })
                        .unwrap();
                }

                // Certain socket states contain cyclic references with other sockets.
                // We need to manually remove these to
                if let SocketState::Connected(connected) = &mut socket_info.borrow_mut().state {
                    if !connected.peer_closed {
                        connected.peer_closed = true;
                        if let ConnectedBackend::Peered(peer_info) = &mut connected.backend {
                            // TODO: do we take the peer's socket ID here so that we can set peer_closed = true on it?
                            // TODO: do we raise the poll of the peer here?
                            peer_info.peer = Weak::new_in(fizzle_alloc());
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

        state.local.fds.remove(&self.fd);
        Outcome::Success(())
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub enum pid_type {
    F_OWNER_TID = 0,
    F_OWNER_PID,
    F_OWNER_PGRP, // F_OWNER_GID
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct f_owner_ex {
    pid_type: pid_type,
    pid: libc::pid_t,
}

pub enum FcntlCommand<'a> {
    DupFd(RawFd),
    DupFdCloexec(RawFd),
    SetFd(RawFd),
    SetFl(libc::c_int),
    SetOwn(libc::c_int),
    SetSig(libc::c_int),
    Notify(libc::c_int),
    SetPipeSize(libc::c_int),
    AddSeals(libc::c_int),
    GetFd,
    GetFl,
    GetOwn,
    GetSig,
    GetLease,
    GetSeals,
    SetLock(&'a mut libc::flock),
    SetLockWait(&'a mut libc::flock),
    GetLock(&'a mut libc::flock),
    OfdSetLock(&'a mut libc::flock),
    OfdSetLockWait(&'a mut libc::flock),
    OfdGetLock(&'a mut libc::flock),
    GetOwnEx(&'a mut f_owner_ex),
    SetOwnEx(&'a mut f_owner_ex),
    GetRwHint(&'a mut u64),
    SetRwHint(&'a mut u64),
    GetFileRwHint(&'a mut u64),
    SetFileRwHint(&'a mut u64),
}

pub struct FcntlEvent<'a> {
    fd: Descriptor,
    command: FcntlCommand<'a>,
}

impl<'a> FcntlEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, command: FcntlCommand<'a>) -> Self {
        Self { fd, command }
    }
}

impl Event for FcntlEvent<'_> {
    type Success = libc::c_int;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match state.local.fds.get_mut(&self.fd) {
            Some(fd_info) if fd_info.is_passthrough => {
                unimplemented!("fd passthrough")
                /*
                let dupfd = libc::fcntl(fd, cmd, arg);
                if dupfd >= 0 && (cmd == libc::F_DUPFD || cmd == libc::F_DUPFD_CLOEXEC) {
                    let nonblocking = fd_info.nonblocking;
                    let close_on_exec = cmd == libc::F_DUPFD_CLOEXEC;
                    let resource = fd_info.resource.clone();
                    state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(dupfd), DescriptorInfo {
                        close_on_exec,
                        nonblocking,
                        is_passthrough: true,
                        resource,
                    }).unwrap();
                }

                dupfd
                */
            }
            Some(fd_info) => {
                match &self.command {
                    FcntlCommand::GetFl => {
                        // TODO: handle other flags
                        if fd_info.nonblocking {
                            Outcome::Success(libc::O_NONBLOCK)
                        } else {
                            Outcome::Success(0)
                        }
                    }
                    FcntlCommand::SetFl(flags) => {
                        fd_info.nonblocking = flags & libc::O_NONBLOCK > 0;
                        Outcome::Success(0)
                    }
                    FcntlCommand::GetFd => {
                        // TODO: handle other fields
                        if fd_info.close_on_exec {
                            Outcome::Success(libc::O_CLOEXEC)
                        } else {
                            Outcome::Success(0)
                        }
                    }
                    FcntlCommand::SetFd(fields) => {
                        fd_info.close_on_exec = fields & libc::O_CLOEXEC > 0; // TODO: can CLOEXEC be unset?
                        Outcome::Success(0)
                    }
                    FcntlCommand::DupFd(newfd) => {
                        let nonblocking = fd_info.nonblocking;
                        let resource = fd_info.resource.clone();
                        let dupfd =
                            unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD, *newfd) };
                        if dupfd < 0 {
                            return Outcome::Error(Errno::get_errno());
                        }
                        state.local.fds.insert(
                            Descriptor::from_raw_fd(dupfd),
                            DescriptorInfo {
                                close_on_exec: false,
                                nonblocking,
                                is_passthrough: false,
                                resource,
                            },
                        );

                        Outcome::Success(dupfd)
                    }
                    FcntlCommand::DupFdCloexec(newfd) => {
                        let nonblocking = fd_info.nonblocking;
                        let resource = fd_info.resource.clone();
                        let dupfd = unsafe {
                            libc::fcntl(self.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, *newfd)
                        };
                        if dupfd < 0 {
                            return Outcome::Error(Errno::get_errno());
                        }
                        state.local.fds.insert(
                            Descriptor::from_raw_fd(dupfd),
                            DescriptorInfo {
                                close_on_exec: true,
                                nonblocking,
                                is_passthrough: false,
                                resource,
                            },
                        );

                        Outcome::Success(dupfd)
                    }
                    _ => {
                        log::error!("unimplemented fcntl command");
                        Outcome::Error(Errno::EINVAL)
                    }
                }
            }
            None => {
                #[cfg(not(feature = "passthroughfs"))]
                return Outcome::Error(Errno::EBADF);
                #[cfg(feature = "passthroughfs")]
                unimplemented!("fcntl() passthrough");
            }
        }
    }
}

pub struct DescriptorDuplicateEvent {
    old_fd: Descriptor,
    new_fd: Option<Descriptor>,
    close_on_exec: bool,
}

impl DescriptorDuplicateEvent {
    #[inline]
    pub fn new(old_fd: Descriptor, new_fd: Option<Descriptor>, close_on_exec: bool) -> Self {
        Self {
            old_fd,
            new_fd,
            close_on_exec,
        }
    }
}

impl Event for DescriptorDuplicateEvent {
    type Success = RawFd;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // Copy over associated data from the old fd
        let Some(mut new_fd_info) = state.local.fds.get_mut(&self.old_fd).cloned() else {
            #[cfg(not(feature = "passthroughfs"))]
            return Outcome::Error(Errno::EBADF);
            #[cfg(feature = "passthroughfs")]
            return match unsafe {
                libc::fcntl(
                    self.old_fd.as_raw_fd(),
                    libc::F_DUPFD,
                    self.new_fd.map(|d| d.as_raw_fd()).unwrap_or(0),
                )
            } {
                fd @ 0.. => Outcome::Success(fd),
                ..=-1 => Outcome::Error(Errno::get_errno()),
            };
        };

        // Create a new, unique file descriptor
        let new_fd = match self.new_fd {
            Some(fd) => fd,
            None => Descriptor::from_raw_fd(crate::create_descriptor()),
        };

        if self.old_fd == new_fd {
            return Outcome::Error(Errno::EINVAL); // Behavior for dup3 (dup2 is different)
        }

        // Update the close-on-exec flag
        new_fd_info.close_on_exec = self.close_on_exec;

        // Upref the file descriptor count where applicable
        if let FdResource::Socket(socket_info) = new_fd_info.resource.clone() {
            socket_info.borrow_mut().fd_count += 1;
        }

        // Close `newfd` if it points to an occupied descriptor
        if state.local.fds.contains_key(&new_fd) {
            // TODO: this is dangerous(ish), as the behavior of the run event loop may change.
            // Figure out how to call events within events more ergonomically while not exposing
            // `ctx`
            match DescriptorCloseEvent::new(new_fd).run(state) {
                Outcome::Success(()) => (),
                Outcome::Error(_) => unreachable!("internal state inconsistency"),
                _ => unreachable!(),
            }
        }

        state.local.fds.insert(new_fd, new_fd_info);

        Outcome::Success(new_fd.as_raw_fd())
    }
}

pub enum ReadData<'a> {
    Basic(&'a mut [IoSliceMut<'a>]),
    File(FileReadData<'a>),
    Socket(&'a mut [SocketReadData<'a>], SocketFlags),
}

pub struct FileReadData<'a> {
    pub buf: &'a mut [IoSliceMut<'a>],
    /// Offset from `pread()` family of functions
    pub offset: Option<libc::off_t>,
    pub flags: FileFlags,
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct FileFlags: libc::c_int {
        const DSYNC = libc::RWF_DSYNC;
        const HIPRI = libc::RWF_HIPRI;
        const SYNC = libc::RWF_SYNC;
        const NOWAIT = libc::RWF_NOWAIT;
        const APPEND = libc::RWF_APPEND;
    }
}

pub struct SocketReadData<'a> {
    pub addr_bytes: &'a mut [MaybeUninit<u8>],
    pub addrlen: &'a mut libc::socklen_t,
    pub buf: &'a mut [IoSliceMut<'a>],
    pub buflen: &'a mut u32,
    pub control_info: &'a mut [MaybeUninit<u8>],
    pub control_len: &'a mut usize,
    pub msg_flags: &'a mut SocketMsgFlags,
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct SocketFlags: libc::c_int {
        const CMSG_CLOEXEC = libc::MSG_CMSG_CLOEXEC;
        const TRUNC = libc::MSG_TRUNC;
        const OOB = libc::MSG_OOB;
        const ERRQUEUE = libc::MSG_ERRQUEUE;
        const DONTWAIT = libc::MSG_DONTWAIT;
        const PEEK = libc::MSG_PEEK;
        const WAITALL = libc::MSG_WAITALL;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct SocketMsgFlags: libc::c_int {
        const EOR = libc::MSG_EOR;
        const TRUNC = libc::MSG_TRUNC;
        const CTRUNC = libc::MSG_CTRUNC;
        const OOB = libc::MSG_OOB;
        const ERRQUEUE = libc::MSG_ERRQUEUE;
        const NOTIFICATION = libc::MSG_NOTIFICATION;
    }
}

enum DescriptorReadState<'a> {
    Start,
    Directory(DirectoryReadEvent<'a>),
    Epoll(EpollReadEvent<'a>),
    Eventfd(EventfdReadEvent<'a>),
    Socket(SocketReadEvent<'a>),
    File(FileReadEvent<'a>),
    Mq(MqReadEvent<'a>),
    Pipe(PipeReadEvent<'a>),
    Stdin(StdinReadEvent<'a>),
}

pub struct DescriptorReadEvent<'a> {
    fd: Descriptor,
    data: Option<ReadData<'a>>,
    state: DescriptorReadState<'a>,
}

impl<'a> DescriptorReadEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: ReadData<'a>) -> Self {
        Self {
            fd,
            data: Some(data),
            state: DescriptorReadState::Start,
        }
    }
}

impl Event for DescriptorReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            DescriptorReadState::Start => {
                let Some(fd_info @ DescriptorInfo { is_passthrough: false, .. }) = state.local.fds.get(&self.fd) else {
                    #[cfg(not(feature = "passthroughfs"))]
                    return Outcome::Error(Errno::EBADF);
                    #[cfg(feature = "passthroughfs")]
                    return match self.data.take().unwrap() {
                        ReadData::Basic(io_slice) => match unsafe {
                            libc::readv(
                                self.fd.as_raw_fd(),
                                io_slice.as_ptr().cast(),
                                io_slice.len() as i32,
                            )
                        } {
                            ..=-1 => Outcome::Error(Errno::get_errno()),
                            len @ 0.. => Outcome::Success(len as usize),
                        },
                        ReadData::File(data) => match unsafe {
                            libc::preadv2(
                                self.fd.as_raw_fd(),
                                data.buf.as_ptr().cast(),
                                data.buf.len() as i32,
                                data.offset.unwrap_or(0),
                                data.flags.bits(),
                            )
                        } {
                            ..=-1 => Outcome::Error(Errno::get_errno()),
                            len @ 0.. => Outcome::Success(len as usize),
                        },
                        ReadData::Socket(msgs, msgflags) => {
                            if msgs.len() != 1 {
                                unimplemented!()
                            }

                            let msg = &mut msgs[0];
                            let mut msghdr = libc::msghdr {
                                msg_name: msg.addr_bytes.as_mut_ptr().cast::<libc::c_void>(),
                                msg_namelen: msg.addr_bytes.len() as u32,
                                msg_iov: msg.buf.as_mut_ptr().cast::<libc::iovec>(),
                                msg_iovlen: msg.buf.len(),
                                msg_control: msg.control_info.as_mut_ptr().cast::<libc::c_void>(),
                                msg_controllen: msg.control_info.len(),
                                msg_flags: msg.msg_flags.bits(),
                            };

                            let ret = unsafe {
                                libc::recvmsg(self.fd.as_raw_fd(), &raw mut msghdr, msgflags.bits())
                            };

                            if ret < 0 {
                                Outcome::Error(Errno::get_errno())
                            } else {
                                *msg.buflen = ret as u32;
                                Outcome::Success(1)
                            }
                        }
                    };
                };

                match &fd_info.resource {
                    FdResource::Directory(_) => {
                        self.state = DescriptorReadState::Directory(DirectoryReadEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Epoll(_) => {
                        self.state = DescriptorReadState::Epoll(EpollReadEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::EventFd(eventfd) => {
                        self.state = DescriptorReadState::Eventfd(EventfdReadEvent::new(
                            eventfd.clone(),
                            fd_info.nonblocking,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::File(_) => {
                        self.state = DescriptorReadState::File(FileReadEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::MessageQueue(_) => {
                        self.state = DescriptorReadState::Mq(MqReadEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Pipe(pipe_info) => {
                        self.state = DescriptorReadState::Pipe(PipeReadEvent::new(
                            pipe_info.clone(),
                            fd_info.nonblocking,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Stdin | FdResource::Stdout | FdResource::Stderr => {
                        // Reading from stdout, stderr is equivalent to reading from stdin
                        self.state = DescriptorReadState::Stdin(StdinReadEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Socket(socket_info) => {
                        self.state = DescriptorReadState::Socket(SocketReadEvent::new(
                            socket_info.clone(),
                            fd_info.nonblocking,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Inotify(_) => unimplemented!(),
                    FdResource::Opaque => unreachable!(),
                }
                Outcome::Yield(YieldUntil::Immediate)
            }
            DescriptorReadState::Directory(e) => e.run(state),
            DescriptorReadState::Epoll(e) => e.run(state),
            DescriptorReadState::Eventfd(e) => e.run(state),
            DescriptorReadState::Socket(e) => e.run(state),
            DescriptorReadState::File(e) => e.run(state),
            DescriptorReadState::Mq(e) => e.run(state),
            DescriptorReadState::Pipe(e) => e.run(state),
            DescriptorReadState::Stdin(e) => e.run(state),
        }
    }
}

pub enum StdinReadState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

pub struct StdinReadEvent<'a> {
    fd: Descriptor,
    data: ReadData<'a>,
    state: StdinReadState,
}

impl<'a> StdinReadEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: ReadData<'a>) -> Self {
        Self {
            fd,
            data,
            state: StdinReadState::Start,
        }
    }
}

impl Event for StdinReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let nonblocking = state.local.fds.get(&self.fd).unwrap().nonblocking;

        let ReadData::Basic(iovec) = &mut self.data else {
            unreachable!(
                "internal error--buffer other than ReadData::Basic passed to StdinReadEent"
            );
        };

        match (&self.state, state.global.stdio.clone()) {
            (_, StdioBackend::Passthrough | StdioBackend::Peered(_)) => unreachable!(),
            (StdinReadState::Start, StdioBackend::Feedback(feedback)) => {
                let read_polled = feedback.read_polled.clone();

                if state.polled_is_ready(&read_polled) {
                    self.state = StdinReadState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), read_polled);

                    self.state = StdinReadState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            (StdinReadState::Finish(_poller_id), StdioBackend::Feedback(_feedback)) => {
                // TODO: remove Feedback entirely? It's been a nuisance...
                unimplemented!()
                /*
                if let Some(poller_id) = poller_id {
                    state.delete_poller(poller_id.clone());
                }

                let read_polled = feedback.read_polled.clone();
                let write_polled = feedback.write_polled.clone();

                let mut total_read = 0;
                let mut read_idx = feedback.read_idx;
                let Some(read_data) = feedback.buf.pop_front() else {
                    unreachable!()
                };

                for slice in iovec.iter_mut() {
                    if read_idx == read_data.len() {
                        break
                    }

                    let data_len = cmp::min(read_data.len() - read_idx, slice.len());
                    slice[..data_len].copy_from_slice(&read_data[read_idx..read_idx + data_len]);

                    read_idx += data_len;
                    total_read += data_len;
                }

                if read_idx == read_data.len() {
                    feedback.read_idx = 0;
                } else {
                    feedback.buf.push_front(read_data);
                    feedback.read_idx = read_idx;
                }

                if feedback.buf.is_empty() {
                    state.lower_polled(&read_polled);
                }
                state.raise_polled(&write_polled);

                Outcome::Success(total_read)
                */
            }
            (StdinReadState::Start, StdioBackend::Plugin(plugin_info)) => {
                let read_polled = plugin_info.borrow().read_polled.clone();

                if state.polled_is_ready(&read_polled) {
                    self.state = StdinReadState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), read_polled);

                    self.state = StdinReadState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            (StdinReadState::Finish(poller), StdioBackend::Plugin(plugin_info)) => {
                if let Some(poller) = poller {
                    state.delete_poller(poller.clone());
                }

                let mut total_read = 0;
                let mut read_idx = plugin_info.borrow_mut().write_idx;
                let Some(read_data) = plugin_info.borrow_mut().write_buf.pop_front() else {
                    unreachable!()
                };

                let read_polled = plugin_info.borrow().read_polled.clone();

                for slice in iovec.iter_mut() {
                    if read_idx == read_data.len() {
                        break;
                    }

                    let data_len = cmp::min(read_data.len() - read_idx, slice.len());
                    slice[..data_len].copy_from_slice(&read_data[read_idx..read_idx + data_len]);

                    read_idx += data_len;
                    total_read += data_len;
                }

                if read_idx == read_data.len() {
                    plugin_info.borrow_mut().write_idx = 0;
                } else {
                    plugin_info.borrow_mut().write_buf.push_front(read_data);
                    plugin_info.borrow_mut().write_idx = read_idx;
                }

                if plugin_info.borrow().write_buf.is_empty() {
                    state.lower_polled(&read_polled);
                }

                Outcome::Success(total_read)
            }
            (_, StdioBackend::Sink) => Outcome::Success(0),
            (_, StdioBackend::NullSink) => {
                let mut total_read = 0;
                for slice in iovec.iter_mut() {
                    for b in slice.iter_mut() {
                        *b = 0;
                    }
                    total_read += slice.len();
                }

                Outcome::Success(total_read)
            }
            (StdinReadState::Start, StdioBackend::Fuzz(fuzz_endpoint)) => {
                let read_polled = fuzz_endpoint.borrow().read_polled.clone();

                if state.polled_is_ready(&read_polled) {
                    self.state = StdinReadState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), read_polled);

                    self.state = StdinReadState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            (StdinReadState::Finish(poller), StdioBackend::Fuzz(fuzz_endpoint)) => {
                if let Some(poller) = poller {
                    state.delete_poller(poller.clone());
                }

                let FuzzEndpointInfo {
                    mut read_idx,
                    read_polled,
                } = fuzz_endpoint.borrow().clone();

                let buf = state.global.fuzz_input.as_slice();
                let buflen = buf.len();

                let mut total_read = 0;
                for slice in iovec.iter_mut() {
                    if buf[read_idx..].is_empty() {
                        break;
                    }

                    let data_len = cmp::min(buf.len(), slice.len());
                    slice[..data_len].copy_from_slice(&buf[read_idx..read_idx + data_len]);

                    read_idx += data_len;
                    total_read += data_len;
                }

                fuzz_endpoint.borrow_mut().read_idx = read_idx;
                if read_idx == buflen {
                    state.lower_polled(&read_polled);
                }

                Outcome::Success(total_read)
            }
        }
    }
}

pub enum WriteData<'a> {
    BasicSlice(&'a [u8]),
    Iovec(&'a [IoSlice<'a>]),
    File(FileWriteData<'a>),
    Socket(&'a mut [SocketWriteData<'a>], SocketFlags),
}

pub struct FileWriteData<'a> {
    pub buf: &'a [IoSlice<'a>],
    /// Offset from `pread()` family of functions
    pub offset: Option<libc::off_t>,
    pub flags: FileFlags,
}

pub struct SocketWriteData<'a> {
    pub addr_bytes: Option<&'a [u8]>,
    pub buf: &'a [IoSlice<'a>],
    pub buflen: &'a mut u32,
    pub control_info: &'a [u8],
    pub msg_flags: SocketMsgFlags,
}

enum DescriptorWriteState<'a> {
    Start,
    Directory(DirectoryWriteEvent<'a>),
    Epoll(EpollWriteEvent<'a>),
    Eventfd(EventfdWriteEvent<'a>),
    Socket(SocketWriteEvent<'a>),
    File(FileWriteEvent<'a>),
    Mq(MqWriteEvent<'a>),
    Pipe(PipeWriteEvent<'a>),
    Stdout(StdoutWriteEvent<'a>),
}

pub struct DescriptorWriteEvent<'a> {
    fd: Descriptor,
    data: Option<WriteData<'a>>,
    state: DescriptorWriteState<'a>,
}

impl<'a> DescriptorWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self {
            fd,
            data: Some(data),
            state: DescriptorWriteState::Start,
        }
    }
}

impl Event for DescriptorWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            DescriptorWriteState::Start => {
                let Some(fd_info @ DescriptorInfo { is_passthrough: false, .. }) = state.local.fds.get(&self.fd) else {
                    #[cfg(not(feature = "passthroughfs"))]
                    return Outcome::Error(Errno::EBADF);
                    #[cfg(feature = "passthroughfs")]
                    return match self.data.take().unwrap() {
                        WriteData::BasicSlice(slice) => match unsafe {
                            libc::write(self.fd.as_raw_fd(), slice.as_ptr().cast(), slice.len())
                        } {
                            ..=-1 => Outcome::Error(Errno::get_errno()),
                            len @ 0.. => Outcome::Success(len as usize),
                        },
                        WriteData::Iovec(io_slice) => match unsafe {
                            libc::writev(
                                self.fd.as_raw_fd(),
                                io_slice.as_ptr().cast(),
                                io_slice.len() as i32,
                            )
                        } {
                            ..=-1 => Outcome::Error(Errno::get_errno()),
                            len @ 0.. => Outcome::Success(len as usize),
                        },
                        WriteData::File(data) => match unsafe {
                            libc::pwritev2(
                                self.fd.as_raw_fd(),
                                data.buf.as_ptr().cast(),
                                data.buf.len() as i32,
                                data.offset.unwrap_or(0),
                                data.flags.bits(),
                            )
                        } {
                            ..=-1 => Outcome::Error(Errno::get_errno()),
                            len @ 0.. => Outcome::Success(len as usize),
                        },
                        WriteData::Socket(msgs, msgflags) => {
                            let mut total_sent = 0;
                            let mut error = None;
                            for msg in msgs {
                                let msghdr = libc::msghdr {
                                    msg_name: msg.addr_bytes.map(|s| s.as_ptr()).unwrap_or(ptr::null()).cast::<libc::c_void>().cast_mut(), // TODO: UB?
                                    msg_namelen: msg.addr_bytes.map(|s| s.len()).unwrap_or(0) as u32,
                                    msg_iov: msg.buf.as_ptr().cast::<libc::iovec>().cast_mut(),
                                    msg_iovlen: msg.buf.len(),
                                    msg_control: msg.control_info.as_ptr().cast::<libc::c_void>().cast_mut(),
                                    msg_controllen: msg.control_info.len(),
                                    msg_flags: msg.msg_flags.bits(),
                                };

                                let ret = unsafe {
                                    libc::sendmsg(self.fd.as_raw_fd(), &raw const msghdr, msgflags.bits())
                                };

                                if ret < 0 {
                                    if error.is_none() {
                                        error = Some(Errno::get_errno());
                                    }
                                } else {
                                    *msg.buflen = ret as u32;
                                    total_sent += 1;
                                }
                            }

                            return if total_sent > 0 {
                                Outcome::Success(total_sent)
                            } else {
                                Outcome::Error(error.unwrap())
                            }
                        },
                    };
                };

                match &fd_info.resource {
                    FdResource::Directory(_) => {
                        self.state = DescriptorWriteState::Directory(DirectoryWriteEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Epoll(_) => {
                        self.state = DescriptorWriteState::Epoll(EpollWriteEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::EventFd(eventfd_id) => {
                        self.state = DescriptorWriteState::Eventfd(EventfdWriteEvent::new(
                            eventfd_id.clone(),
                            fd_info.nonblocking,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::File(_) => {
                        self.state = DescriptorWriteState::File(FileWriteEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::MessageQueue(_) => {
                        self.state = DescriptorWriteState::Mq(MqWriteEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Pipe(pipe_info) => {
                        self.state = DescriptorWriteState::Pipe(PipeWriteEvent::new(
                            pipe_info.clone(),
                            fd_info.nonblocking,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Stdin | FdResource::Stdout => {
                        // Writing to stdin is equivalent to writing to stdout
                        self.state = DescriptorWriteState::Stdout(StdoutWriteEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Stderr => {
                        self.state = DescriptorWriteState::Stdout(StdoutWriteEvent::new(
                            self.fd,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Socket(socket_info) => {
                        self.state = DescriptorWriteState::Socket(SocketWriteEvent::new(
                            socket_info.clone(),
                            fd_info.nonblocking,
                            self.data.take().unwrap(),
                        ));
                    }
                    FdResource::Inotify(_) => unimplemented!("inotify write()"),
                    FdResource::Opaque => unreachable!(),
                }
                Outcome::Yield(YieldUntil::Immediate)
            }
            DescriptorWriteState::Directory(e) => e.run(state),
            DescriptorWriteState::Epoll(e) => e.run(state),
            DescriptorWriteState::Eventfd(e) => e.run(state),
            DescriptorWriteState::Socket(e) => e.run(state),
            DescriptorWriteState::File(e) => e.run(state),
            DescriptorWriteState::Mq(e) => e.run(state),
            DescriptorWriteState::Pipe(e) => e.run(state),
            DescriptorWriteState::Stdout(e) => e.run(state),
        }
    }
}

pub enum StdoutWriteState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

pub struct StdoutWriteEvent<'a> {
    fd: Descriptor,
    data: WriteData<'a>,
    state: StdoutWriteState,
}

impl<'a> StdoutWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self {
            fd,
            data,
            state: StdoutWriteState::Start,
        }
    }
}

impl Event for StdoutWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let nonblocking = state.local.fds.get(&self.fd).unwrap().nonblocking;

        let WriteData::Iovec(iovec) = self.data else {
            unreachable!(
                "internal error--buffer other than WriteData::Basic passed to StdoutWriteEvent"
            );
        };

        match (&self.state, state.global.stdio.clone()) {
            (_, StdioBackend::Passthrough) => {
                log::info!("Data written to stdout"); // TODO: include actual data

                let res = unsafe {
                    libc::writev(2, iovec.as_ptr().cast::<libc::iovec>(), iovec.len() as i32)
                };
                match res {
                    0.. => Outcome::Success(res as usize),
                    _ => Outcome::Error(Errno::get_errno()),
                }
            }
            (_, StdioBackend::Peered(_)) => unreachable!(),
            (StdoutWriteState::Start, crate::backend::IoBackend::Feedback(feedback)) => {
                let write_polled = feedback.write_polled.clone();

                if state.polled_is_ready(&write_polled) {
                    self.state = StdoutWriteState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), write_polled);

                    self.state = StdoutWriteState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            (StdoutWriteState::Finish(_poller_id), StdioBackend::Feedback(_feedback)) => {
                unimplemented!()
                /*
                if let Some(poller_id) = poller_id {
                    state.delete_poller(poller_id.clone());
                }

                let buf = feedback.buf.clone();
                let write_polled = feedback.write_polled.clone();
                let read_polled = feedback.read_polled.clone();

                let mut total_written = 0;
                for slice in iovec {
                    if buf.borrow().is_full() {
                        break;
                    }
                    total_written += buf.borrow_mut().write(slice);
                }

                if buf.borrow().is_full() {
                    state.lower_polled(&write_polled);
                }
                state.raise_polled(&read_polled);

                Outcome::Success(total_written)
                */
            }
            (StdoutWriteState::Start, StdioBackend::Plugin(plugin_info)) => {
                let write_polled = plugin_info.borrow().write_polled.clone();

                if state.polled_is_ready(&write_polled) {
                    self.state = StdoutWriteState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), write_polled);

                    self.state = StdoutWriteState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            (StdoutWriteState::Finish(poller), StdioBackend::Plugin(plugin_info)) => {
                if let Some(poller) = poller {
                    state.delete_poller(poller.clone());
                }

                let mut buf = Vec::new_in(fizzle_alloc());

                let mut total_written = 0;
                for slice in iovec {
                    buf.extend_from_slice(slice);
                    total_written += slice.len();
                }

                plugin_info.borrow_mut().write_buf.push_back(buf);

                Outcome::Success(total_written)
            }
            (_, StdioBackend::Sink) => {
                let total_len = iovec.iter().map(|s| s.len()).sum();
                Outcome::Success(total_len)
            }
            (_, StdioBackend::NullSink) => {
                let total_len = iovec.iter().map(|s| s.len()).sum();
                Outcome::Success(total_len)
            }
            (_, StdioBackend::Fuzz(_)) => {
                let total_len = iovec.iter().map(|s| s.len()).sum();
                Outcome::Success(total_len)
            }
        }
    }
}

pub enum StderrWriteState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

pub struct StderrWriteEvent<'a> {
    fd: Descriptor,
    data: WriteData<'a>,
    state: StderrWriteState,
}

impl<'a> StderrWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self {
            fd,
            data,
            state: StderrWriteState::Start,
        }
    }
}

impl Event for StderrWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let WriteData::Iovec(iovec) = self.data else {
            unreachable!(
                "internal error--buffer other than WriteData::Basic passed to StderrWriteEent"
            );
        };

        if state.global.mask_stderr {
            let total = iovec.iter().map(|s| s.len()).sum();
            Outcome::Success(total)
        } else {
            let res = unsafe {
                libc::writev(2, iovec.as_ptr().cast::<libc::iovec>(), iovec.len() as i32)
            };
            match res {
                0.. => Outcome::Success(res as usize),
                _ => Outcome::Error(Errno::get_errno()),
            }
        }
    }
}
