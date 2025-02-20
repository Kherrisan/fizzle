use std::cell::RefCell;
use std::cmp;
use std::rc::{Rc, Weak};

use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::errno::Errno;
use crate::scheduler::{fizzle_alloc, Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::{GlobalRc, GlobalWeak};

use bitflags::bitflags;

use fizzle_common::storage::Buffer;

use super::descriptor::{Descriptor, DescriptorInfo, FdResource, ReadData, WriteData};
use super::polled::PolledInfo;
use super::poller::PollerInfo;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PipeId(usize);
}

pub struct PipeInfo {
    /// The transmission mode of the packet.
    ///
    /// See [`PipeMode`] for more details.
    pub mode: PipeMode,
    /// The peer pipe that this pipe is connected to.
    ///
    /// If this value is `None`, then the pipe has broken (e.g., the other end has shut).
    pub peer: GlobalWeak<PipeInfo>,
    /// The buffer this pipe reads in data from.
    pub read_buf: GlobalRc<Buffer<FIZZLE_BUFFER_LENGTH>>,
    pub read_polled: GlobalRc<PolledInfo>,
    pub write_polled: GlobalRc<PolledInfo>,
}

/// The mode of operation by which data is passed over the pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeMode {
    /// Performs I/O in "packet" mode--writes are treated as individual packets.
    Direct,
    /// Performs I/O as if data is a constant stream.
    Streamed,
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct PipeCreateFlags: libc::c_int {
        const CLOEXEC = libc::O_CLOEXEC;
        const DIRECT = libc::O_DIRECT;
        const NONBLOCK = libc::O_NONBLOCK;
//        const NOTIFICATION = libc::O_NOTIFICATION_PIPE;
    }
}

pub struct PipeCreateEvent {
    flags: PipeCreateFlags,
}

impl PipeCreateEvent {
    pub fn new(flags: PipeCreateFlags) -> Self {
        Self { flags }
    }
}

impl Event for PipeCreateEvent {
    type Success = (Descriptor, Descriptor);
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let nonblocking = self.flags.contains(PipeCreateFlags::NONBLOCK);
        let close_on_exec = self.flags.contains(PipeCreateFlags::CLOEXEC);
        let mode = if self.flags.contains(PipeCreateFlags::DIRECT) {
            PipeMode::Direct
        } else {
            PipeMode::Streamed
        };

        let fd1 = crate::create_descriptor();
        let fd2 = crate::create_descriptor();

        let first_pipe = Rc::new_in(
            RefCell::new(PipeInfo {
                mode,
                peer: Weak::new_in(fizzle_alloc()),
                read_buf: Rc::new_in(RefCell::new(Buffer::new()), fizzle_alloc()),
                read_polled: Rc::new_in(
                    RefCell::new(PolledInfo {
                        pollers: Vec::new_in(fizzle_alloc()),
                        event_raised: false,
                    }),
                    fizzle_alloc(),
                ),
                write_polled: Rc::new_in(
                    RefCell::new(PolledInfo {
                        pollers: Vec::new_in(fizzle_alloc()),
                        event_raised: true,
                    }),
                    fizzle_alloc(),
                ),
            }),
            fizzle_alloc(),
        );

        let second_pipe = Rc::new_in(
            RefCell::new(PipeInfo {
                mode,
                peer: Rc::downgrade(&first_pipe),
                read_buf: Rc::new_in(RefCell::new(Buffer::new()), fizzle_alloc()),
                read_polled: Rc::new_in(
                    RefCell::new(PolledInfo {
                        pollers: Vec::new_in(fizzle_alloc()),
                        event_raised: false,
                    }),
                    fizzle_alloc(),
                ),
                write_polled: Rc::new_in(
                    RefCell::new(PolledInfo {
                        pollers: Vec::new_in(fizzle_alloc()),
                        event_raised: true,
                    }),
                    fizzle_alloc(),
                ),
            }),
            fizzle_alloc(),
        );

        // `unwrap()` guaranteed to succeed--we *just* inserted the pipe
        first_pipe.borrow_mut().peer = Rc::downgrade(&second_pipe);

        let fd1_info = DescriptorInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Pipe(first_pipe),
        };

        let fd2_info = DescriptorInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Pipe(second_pipe),
        };

        let desc1 = Descriptor::from_raw_fd(fd1);
        let desc2 = Descriptor::from_raw_fd(fd2);

        // Now add the fd -> pipe_id mapping
        state.local.fds.insert(desc1, fd1_info);
        state.local.fds.insert(desc2, fd2_info);

        Outcome::Success((desc1, desc2))
    }
}

pub enum PipeReadState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

pub struct PipeReadEvent<'a> {
    pipe_info: GlobalRc<PipeInfo>,
    nonblocking: bool,
    data: ReadData<'a>,
    state: PipeReadState,
}

impl<'a> PipeReadEvent<'a> {
    #[inline]
    pub fn new(pipe_info: GlobalRc<PipeInfo>, nonblocking: bool, data: ReadData<'a>) -> Self {
        Self {
            pipe_info,
            nonblocking,
            data,
            state: PipeReadState::Start,
        }
    }
}

impl Event for PipeReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let ReadData::Basic(iovec) = &mut self.data else {
            unreachable!(
                "internal error--buffer other than ReadData::Basic passed to PipeReadEvent"
            );
        };

        match &self.state {
            PipeReadState::Start => {
                let peer_is_closed = self.pipe_info.borrow().peer.upgrade().is_none();
                let read_polled = self.pipe_info.borrow().read_polled.clone();

                if state.polled_is_ready(&read_polled) {
                    self.state = PipeReadState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if peer_is_closed {
                    Outcome::Success(0)
                } else if self.nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), read_polled.clone());

                    self.state = PipeReadState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            PipeReadState::Finish(poller) => {
                if let Some(poller) = poller {
                    state.delete_poller(poller.clone());
                }

                let peer_is_closed = self.pipe_info.borrow().peer.upgrade().is_none();
                let pipe_mode = self.pipe_info.borrow().mode;

                let buf = self.pipe_info.borrow().read_buf.clone();
                let mut buf_mut = buf.borrow_mut();
                let write_polled = self.pipe_info.borrow().write_polled.clone();
                let read_polled = self.pipe_info.borrow().read_polled.clone();

                if buf.borrow().is_empty() {
                    assert!(peer_is_closed);
                    // TODO: sigpipe to self?
                    return Outcome::Success(0);
                }

                let total_read = match pipe_mode {
                    PipeMode::Direct => {
                        let mut packet_len_bytes = [0u8; 2];
                        assert_eq!(buf_mut.read(packet_len_bytes.as_mut_slice()), 2);
                        let packet_len = u16::from_be_bytes(packet_len_bytes) as usize;

                        let packet = &buf_mut.data()[..packet_len];
                        let mut total_read = 0;
                        for slice in iovec.iter_mut() {
                            let v_read = cmp::min(packet.len() - total_read, slice.len());
                            slice.copy_from_slice(&packet[total_read..total_read + v_read]);
                            total_read += v_read;
                        }

                        buf.borrow_mut().did_read(total_read);
                        total_read
                    }
                    PipeMode::Streamed => {
                        let packet = buf_mut.data();
                        let mut total_read = 0;
                        for slice in iovec.iter_mut() {
                            let v_read = cmp::min(packet.len() - total_read, slice.len());
                            slice.copy_from_slice(&packet[total_read..total_read + v_read]);
                            total_read += v_read;
                        }

                        buf_mut.did_read(total_read);

                        total_read
                    }
                };

                if buf.borrow().is_empty() {
                    state.lower_polled(&read_polled);
                }
                state.raise_polled(&write_polled);

                Outcome::Success(total_read)
            }
        }
    }
}

enum PipeWriteState {
    Start,
    NextPayload(Option<GlobalRc<PollerInfo>>),
}

pub struct PipeWriteEvent<'a> {
    pipe_info: GlobalRc<PipeInfo>,
    nonblocking: bool,
    data: WriteData<'a>,
    data_start: (usize, usize),
    data_written: usize,
    state: PipeWriteState,
}

impl<'a> PipeWriteEvent<'a> {
    #[inline]
    pub fn new(pipe_info: GlobalRc<PipeInfo>, nonblocking: bool, data: WriteData<'a>) -> Self {
        Self {
            pipe_info,
            nonblocking,
            data,
            data_start: (0, 0),
            data_written: 0,
            state: PipeWriteState::Start,
        }
    }
}

impl Event for PipeWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let WriteData::BasicVec(iovec) = &self.data else {
            unreachable!(
                "internal error--buffer other than WriteData::Basic passed to PipeWriteEvent"
            );
        };
        let total_len: usize = iovec.iter().map(|s| s.len()).sum();
        let remaining_len = total_len - self.data_written;

        match &self.state {
            PipeWriteState::Start => {
                let write_polled = self.pipe_info.borrow().write_polled.clone();

                let Some(peer_info) = self.pipe_info.borrow().peer.upgrade() else {
                    // TODO: send signal here?
                    return Outcome::Error(Errno::EPIPE);
                };

                let peer_buf = peer_info.borrow().read_buf.clone();
                let peer_mode = peer_info.borrow().mode;

                if peer_buf.borrow().remaining_len() >= 2 + cmp::min(libc::PIPE_BUF, remaining_len)
                    || peer_mode == PipeMode::Streamed && state.polled_is_ready(&write_polled)
                {
                    self.state = PipeWriteState::NextPayload(None);
                    return Outcome::Yield(YieldUntil::Immediate);
                }

                // Not enough data for buffer--enable polled to be raised again once more data read.
                state.lower_polled(&write_polled);

                if self.nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), write_polled.clone());

                    self.state = PipeWriteState::NextPayload(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            PipeWriteState::NextPayload(poller) => {
                if let Some(poller) = poller {
                    state.delete_poller(poller.clone());
                }

                let write_polled = self.pipe_info.borrow().write_polled.clone();
                let Some(peer) = self.pipe_info.borrow().peer.upgrade() else {
                    // TODO: send signal here?
                    return Outcome::Error(Errno::EPIPE);
                };

                let read_polled = peer.borrow().read_polled.clone();
                let buf = peer.borrow().read_buf.clone();
                let pipe_mode = peer.borrow().mode;

                if pipe_mode == PipeMode::Direct
                    && buf.borrow().remaining_len() < 2 + cmp::min(libc::PIPE_BUF, remaining_len)
                {
                    // Invariant: a nonblocking socket will never continue to this state unless
                    // there is sufficient data
                    assert!(!self.nonblocking);

                    // Some data was read, but it wasn't enough to free up the buffer for a packet--keep waiting
                    state.lower_polled(&write_polled);

                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), write_polled.clone());

                    self.state = PipeWriteState::NextPayload(Some(poller_id));
                    return Outcome::Yield(YieldUntil::None);
                }

                let total_written = match pipe_mode {
                    PipeMode::Direct => {
                        let payload_len = cmp::min(remaining_len, libc::PIPE_BUF);
                        let payload_len_bytes = (payload_len as u16).to_be_bytes();

                        assert_eq!(buf.borrow_mut().write(payload_len_bytes.as_slice()), 2);

                        let mut total_written = 0;

                        for (idx, slice) in iovec[self.data_start.0..].iter().enumerate() {
                            let slice = if idx == 0 {
                                &slice[self.data_start.1..]
                            } else {
                                &slice
                            };

                            let cap = cmp::min(payload_len - total_written, slice.len());
                            let written = buf.borrow_mut().write(&slice[..cap]);
                            total_written += written;

                            if written < slice.len() {
                                self.data_start = (self.data_start.0 + idx, written);
                            }
                        }

                        total_written
                    }
                    PipeMode::Streamed => {
                        let mut total_written = 0;
                        for slice in iovec.iter() {
                            let written = buf.borrow_mut().write(slice);
                            total_written += written;
                        }

                        total_written
                    }
                };

                self.data_written += total_written;

                let remaining = total_len - self.data_written;

                let buf_is_full = match pipe_mode {
                    PipeMode::Direct => {
                        buf.borrow_mut().remaining_len() < 2 + cmp::min(libc::PIPE_BUF, remaining)
                    }
                    PipeMode::Streamed => buf.borrow().is_full(),
                };

                if buf_is_full {
                    state.lower_polled(&write_polled);
                }
                state.raise_polled(&read_polled);

                if pipe_mode == PipeMode::Direct && remaining > 0 {
                    if !buf_is_full {
                        self.state = PipeWriteState::NextPayload(None);
                        Outcome::Yield(YieldUntil::Immediate)
                    } else if self.nonblocking {
                        Outcome::Success(self.data_written)
                    } else {
                        let poller_id = state.new_poller();
                        state.register_poller(poller_id.clone(), write_polled.clone());

                        self.state = PipeWriteState::NextPayload(Some(poller_id));
                        Outcome::Yield(YieldUntil::None)
                    }
                } else {
                    Outcome::Success(self.data_written)
                }
            }
        }

        /*
        let Some(peer_id) = state.global.pipes.get(self).unwrap().peer.clone() else {
            // TODO: send signal here?
            return Outcome::Error(Errno::EPIPE)
        };

        let peer_info = state.global.pipes.get(&peer_id).unwrap();
        let buffer_id = peer_info.read_buf.clone();
        let write_polled = peer_info.write_polled.clone();
        let read_polled = peer_info.read_polled.clone();

        let pipe_mode = peer_info.mode;

        let polled_is_ready = state.polled_is_ready(&write_polled);
        drop(state);
        if !polled_is_ready {
            if nonblocking {
                return Err(PipeError::WouldBlock);
            } else {
                ctx.poll_until_ready(write_polled.clone());
            }
        }


        // We need to verify that this connection has not shut down before writing to the same buffer_id
        if state.global.pipes.get(self).unwrap().peer.is_none() {
            return Err(PipeError::PipeClosed);
        };
        */
    }
}
