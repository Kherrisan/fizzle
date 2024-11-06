use std::cmp;
use std::fmt::Display;
use std::os::fd::RawFd;

use bitflags::bitflags;
use fizzle_common::storage::Buffer;

use crate::arena::{ArenaKey, Rc};
use crate::backend::FileBackend;
use crate::constants::FIZZLE_FOPEN_BUFSIZE;
use crate::state::FizzleSingleton;

use super::descriptor::{DescriptorError, DescriptorId};
use super::fuzz_endpoint::FuzzEndpointInfo;
use super::{init_from_slice, MsgHdr, MsgHdrOut};

pub use private::FileId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FileId(usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePtr(usize);

impl Rc<FileId> {
    pub fn read(&self, ctx: &mut FizzleSingleton, msg: &mut MsgHdrOut) -> Result<usize, FileError> {
        let mut state = ctx.acquire();

        match state.global.files.get(self).unwrap() {
            FileBackend::Passthrough | FileBackend::Peered(_) => unreachable!(),
            FileBackend::Feedback(feedback) => {
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
                    ctx.poll_until_ready(read_polled.clone());
                }

                let mut state = ctx.acquire();

                let buf = state.global.buffers.get_mut(&buffer_id).unwrap();

                let mut total_read = 0;
                for iovec in msg.vdata_mut() {
                    if buf.is_empty() {
                        break;
                    }
                    total_read += buf.read_uninit(iovec.data_mut());
                }

                if buf.is_empty() {
                    state.lower_polled(&read_polled);
                }
                state.raise_polled(&write_polled);

                Ok(total_read)
            }
            FileBackend::Plugin(plugin_id) => {
                let plugin_id = plugin_id.clone();
                let plugin_info = state.global.plugins.get(&plugin_id).unwrap();
                let buffer_id = plugin_info.read_buf.clone();
                let read_polled = plugin_info.read_polled.clone();

                let event_raised = state
                    .global
                    .polled_events
                    .get(&read_polled)
                    .unwrap()
                    .event_raised;
                drop(state);

                if !event_raised {
                    ctx.poll_until_ready(read_polled.clone());
                }

                let mut state = ctx.acquire();

                let buf = state.global.buffers.get_mut(&buffer_id).unwrap();

                let mut total_read = 0;
                for iovec in msg.vdata_mut() {
                    if buf.is_empty() {
                        break;
                    }
                    total_read += buf.read_uninit(iovec.data_mut());
                }

                if buf.is_empty() {
                    state.lower_polled(&read_polled);
                }

                Ok(total_read)
            }
            FileBackend::Sink => Ok(0),
            FileBackend::NullSink => {
                let mut total_read = 0;
                for iovec in msg.vdata_mut() {
                    for b in iovec.data_mut() {
                        b.write(0);
                    }
                    total_read += iovec.data_mut().len();
                }

                Ok(total_read)
            }
            FileBackend::Fuzz(fuzz_endpoint_id) => {
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
                    ctx.poll_until_ready(read_polled.clone());
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

    pub fn write(&self, ctx: &mut FizzleSingleton, msg: &impl MsgHdr) -> Result<usize, FileError> {
        let total_len = msg.vdata().iter().map(|v| v.data().len()).sum();

        let state = ctx.acquire();

        match state.global.files.get(self).unwrap() {
            FileBackend::Passthrough | FileBackend::Peered(_) => unreachable!(),
            FileBackend::Feedback(feedback) => {
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
                    ctx.poll_until_ready(write_polled.clone());
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
            FileBackend::Plugin(plugin_id) => {
                let plugin_id = plugin_id.clone();
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
                    ctx.poll_until_ready(write_polled.clone());
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

                return Ok(total_written);
            }
            FileBackend::Sink => Ok(total_len),
            FileBackend::NullSink => Ok(total_len),
            FileBackend::Fuzz(_) => Ok(total_len),
        }
    }
}

impl From<*mut libc::FILE> for FilePtr {
    fn from(value: *mut libc::FILE) -> Self {
        FilePtr(value as usize)
    }
}

impl FilePtr {
    pub fn flush(&self, ctx: &mut FizzleSingleton) -> Result<(), FileError> {
        let state = ctx.acquire();

        let Some(file_obj) = state.local.file_objs.get(self) else {
            return Err(FileError::InvalidPtr);
        };

        let _descriptor_id = DescriptorId::from_raw_fd(file_obj.fd);

        todo!()
    }

    pub fn close(&self, ctx: &mut FizzleSingleton) -> Result<FileObject, FileError> {
        self.flush(ctx)?;

        let mut state = ctx.acquire();

        match state.local.file_objs.remove(self) {
            Some(file_info) => Ok(file_info),
            None => Err(FileError::InvalidPtr),
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AccessMode: libc::mode_t {
        const SUID_BIT = libc::S_ISUID;
        const SGID_BIT = libc::S_ISGID;
        const STICKY_BIT = libc::S_ISVTX;

        const USER_READ = libc::S_IRUSR;
        const USER_WRITE = libc::S_IWUSR;
        const USER_EXEC = libc::S_IXUSR;

        const GROUP_READ = libc::S_IRGRP;
        const GROUP_WRITE = libc::S_IWGRP;
        const GROUP_EXEC = libc::S_IXGRP;

        const OTHER_READ = libc::S_IROTH;
        const OTHER_WRITE = libc::S_IWOTH;
        const OTHER_EXEC = libc::S_IXOTH;
    }
}

impl Display for AccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}{}{}{}", (self.bits() >> 9) & 7, (self.bits() >> 6) & 7, (self.bits() >> 3) & 7, self.bits() & 7))
    }
}

#[derive(Debug)]
pub enum FileError {
    /// The supplied FILE* did not correspond to an existing entry in the Fizzle state.
    InvalidPtr,
    /// The underlying file pointed to by the stream was not open, or not open for the intended mode.
    BadFile,
    /// A socket-specific operation was attempted on the file.
    NotSocket,
}

impl From<FileError> for DescriptorError {
    fn from(value: FileError) -> Self {
        match value {
            FileError::InvalidPtr => DescriptorError::InvalidInput,
            FileError::BadFile => DescriptorError::BadFd,
            FileError::NotSocket => DescriptorError::NotSocket,
        }
    }
}

impl Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidPtr => "InvalidPtr",
            Self::BadFile => "BadFile",
            Self::NotSocket => "NotSocket",
        })
    }
}

impl FileError {
    pub fn as_os_error(&self) -> i32 {
        match self {
            Self::InvalidPtr => libc::EFAULT,
            Self::BadFile => libc::EBADF,
            Self::NotSocket => libc::ENOTSOCK,
        }
    }
}

#[derive(Debug)]
pub struct FileObject {
    pub fd: RawFd,
    pub read_buf: Buffer<FIZZLE_FOPEN_BUFSIZE>,
    pub write_buf: Buffer<FIZZLE_FOPEN_BUFSIZE>,
}

impl FileObject {
    pub fn new(fd: RawFd) -> Self {
        Self {
            fd,
            read_buf: Buffer::new(),
            write_buf: Buffer::new(),
        }
    }
}

impl ArenaKey for FileId {
    type Value = FileBackend;
}
