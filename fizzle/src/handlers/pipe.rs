use std::{cmp, mem};

use crate::arena::{ArenaKey, Rc};
use crate::state::FizzleSingleton;

pub use private::PipeId;

use super::buffer::BufferId;
use super::descriptor::DescriptorError;
use super::polled::PolledId;
use super::{MsgHdr, MsgHdrOut};

const PIPE_BUF: usize = 8192;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PipeId(usize);
}

#[derive(Debug)]
pub struct PipeInfo {
    /// The transmission mode of the packet.
    ///
    /// See [`PipeMode`] for more details.
    pub mode: PipeMode,
    /// The peer pipe that this pipe is connected to.
    ///
    /// If this value is `None`, then the pipe has broken (e.g., the other end has shut).
    pub peer: Option<Rc<PipeId>>,
    /// The buffer this pipe reads in data from.
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
}

/// The mode of operation by which data is passed over the pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeMode {
    /// Performs I/O in "packet" mode--writes are treated as individual packets.
    Direct,
    /// Performs I/O as if data is a constant stream.
    Streamed,
}

impl ArenaKey for PipeId {
    type Value = PipeInfo;
}

impl PipeId {
    pub fn read(&self, ctx: &mut FizzleSingleton, msg: &mut MsgHdrOut, nonblocking: bool) -> Result<usize, PipeError> {
        let mut state = ctx.acquire();

        let pipe_info = state.global.pipes.get(self).unwrap();
        let peer_is_closed = pipe_info.peer.is_none();

        let buffer_id = pipe_info.read_buf.clone();
        let write_polled = pipe_info.write_polled.clone();
        let read_polled = pipe_info.read_polled.clone();

        let pipe_mode = pipe_info.mode;
        let polled_is_ready = state.polled_is_ready(&write_polled);
        drop(state);

        if !polled_is_ready {
            if peer_is_closed {
                return Ok(0)
            } else if nonblocking {
                return Err(PipeError::WouldBlock)
            } else {
                ctx.poll_until_ready(write_polled.clone());
            }
        }

        let mut state = ctx.acquire();

        if state.global.pipes.get(self).unwrap().peer.is_none() {
            return Ok(0)
        }

        let buf = state.global.buffers.get_mut(&buffer_id).unwrap();
        let total_read = match pipe_mode {
            PipeMode::Direct => {
                let mut packet_len_bytes = [0u8; 2];
                assert_eq!(buf.read(&mut packet_len_bytes), 2);
                let packet_len = u16::from_be_bytes(packet_len_bytes) as usize;

                let packet = &buf.data()[..packet_len];
                let total_read = super::read_stream(msg, packet);
                buf.did_read(total_read);

                total_read
            },
            PipeMode::Streamed => {
                let packet = buf.data();
                let total_read = super::read_stream(msg, packet);
                buf.did_read(total_read);

                total_read
            },
        };

        if buf.is_empty() {
            state.lower_polled(&read_polled);
        }
        state.raise_polled(&write_polled);

        Ok(total_read)
    }

    pub fn write(&self, ctx: &mut FizzleSingleton, msg: &impl MsgHdr, nonblocking: bool) -> Result<usize, PipeError> {
        let mut state = ctx.acquire();

        let Some(peer_id) = state.global.pipes.get(self).unwrap().peer.clone() else {
            return Err(PipeError::PipeClosed)
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
                return Err(PipeError::WouldBlock)
            } else {
                ctx.poll_until_ready(write_polled.clone());
            }
        }

        let mut state = ctx.acquire();

        // We need to verify that this connection has not shut down before writing to the same buffer_id
        if state.global.pipes.get(self).unwrap().peer.is_none() {
            return Err(PipeError::PipeClosed)
        };

        let buf = state.global.buffers.get_mut(&buffer_id).unwrap();
        let total_written = match pipe_mode {
            PipeMode::Direct => {
                let packet_len = msg.vdata().iter().map(|v| v.data().len()).sum::<usize>() as u16;
                let packet_len_bytes = packet_len.to_be_bytes();

                assert_eq!(buf.write(packet_len_bytes.as_slice()), 2);

                let mut total_written = 0;
                for iovec in msg.vdata() {
                    let cap = cmp::min(PIPE_BUF - total_written, iovec.data().len());
                    total_written += buf.write(&iovec.data()[..cap]);
                }
                
                total_written
            },
            PipeMode::Streamed => {
                let mut total_written = 0;
                for iovec in msg.vdata() {
                    total_written += buf.write(iovec.data());
                }

                total_written
            },
        };

        let buf_is_full = match pipe_mode {
            PipeMode::Direct => buf.write_available() < PIPE_BUF + mem::size_of::<u16>(),
            PipeMode::Streamed => buf.is_full(),
        };

        if buf_is_full {
            state.lower_polled(&write_polled);
        }
        state.raise_polled(&read_polled);

        Ok(total_written)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PipeError {
    /// A socket-specific operation was attempted on the pipe.
    NotSocket,
    /// A non-blocking operation would lead to blocking.
    WouldBlock,
    /// The peer of a pipe has closed such that writes fail.
    PipeClosed,
}

impl From<PipeError> for DescriptorError {
    fn from(value: PipeError) -> Self {
        match value {
            PipeError::NotSocket => Self::NotSocket,
            PipeError::WouldBlock => Self::WouldBlock,
            PipeError::PipeClosed => Self::PipeClosed,
        }
    }
}

