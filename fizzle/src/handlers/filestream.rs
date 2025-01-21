use core::slice;
use std::cell::Cell;
use std::ffi::CStr;
use std::io::{IoSlice, IoSliceMut};
use std::{cmp, mem};
use std::os::fd::RawFd;
use std::ptr::NonNull;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use super::descriptor::*;
use super::file::FileOpenFlags;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePtr(NonNull<libc::FILE>);

impl FilePtr {
    pub fn from_raw(value: *mut libc::FILE) -> Option<Self> {
        Some(FilePtr(NonNull::new(value)?))
    }

    pub fn as_raw(&mut self) -> *mut libc::FILE {
        self.0.as_ptr()
    }
}

pub enum FileStreamSource {
    Descriptor(RawFd),
    Slice(Cell<NonNull<[u8]>>, usize),
    Buffer(Cell<Vec<u8>>, usize),
}

// TODO: for now, we just pass everything through--none of these buffers are actually used.
// Need to fix this to emulate proper buffering/flushing
pub enum FileStreamBuffer {
    Internal(Box<[u8]>),
    Slice(NonNull<[u8]>),
    None,
}

pub struct FileStreamMode {
    pub flags: FileOpenFlags,
    pub input_mode: FileInputMode,
    pub no_cancellation: bool,
    pub cloexec: bool,
    pub read_mmap: bool,
    pub exclusive_create: bool,
    pub charset: Option<String>,
}

pub enum FileInputMode {
    Binary,
    Text,
}

impl FileStreamMode {
    pub fn from_cstr(mode: &CStr) -> Option<Self> {
        let mut bytes = mode.to_bytes().iter().map(|b| *b).peekable();
        let mut no_cancellation = false;
        let mut cloexec = false;
        let mut read_mmap = false;
        let mut exclusive_create = false;
        let mut charset = None;
        let mut input_mode = FileInputMode::Text;

        let flags = match bytes.next()? {
            b'r' => {
                match bytes.peek() {
                    Some(&b'b') => {
                        bytes.next();
                        input_mode = FileInputMode::Binary;
                    }
                    Some(&b't') => {
                        bytes.next();
                        input_mode = FileInputMode::Text;
                    }
                    _ => (),
                }

                if bytes.peek() == Some(&b'+') {
                    bytes.next();
                    FileOpenFlags::READWRITE
                } else {
                    FileOpenFlags::empty() // READONLY
                }
            }
            b'w' => {
                match bytes.peek() {
                    Some(&b'b') => {
                        bytes.next();
                        input_mode = FileInputMode::Binary;
                    }
                    Some(&b't') => {
                        bytes.next();
                        input_mode = FileInputMode::Text;
                    }
                    _ => (),
                }

                if bytes.peek() == Some(&b'+') {
                    bytes.next();
                    FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::TRUNC
                } else {
                    FileOpenFlags::WRITEONLY | FileOpenFlags::CREATE | FileOpenFlags::TRUNC
                }
            }
            b'a' => {
                match bytes.peek() {
                    Some(&b'b') => {
                        bytes.next();
                        input_mode = FileInputMode::Binary;
                    }
                    Some(&b't') => {
                        bytes.next();
                        input_mode = FileInputMode::Text;
                    }
                    _ => (),
                }

                if bytes.peek() == Some(&b'+') {
                    bytes.next();
                    FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::APPEND
                } else {
                    FileOpenFlags::WRITEONLY | FileOpenFlags::CREATE | FileOpenFlags::APPEND
                }
            }
            _ => return None,
        };

        while let Some(b) = bytes.next() {
            match b {
                b'c' if no_cancellation => return None,
                b'c' => no_cancellation = true,
                b'e' if cloexec => return None,
                b'e' => cloexec = true,
                b'm' if read_mmap => return None,
                b'm' => read_mmap = true,
                b'x' if exclusive_create => return None,
                b'x' => exclusive_create = true,
                b',' if charset.is_some() => return None,
                b',' => {
                    bytes.next().filter(|b| b == &b'c')?;
                    bytes.next().filter(|b| b == &b'c')?;
                    bytes.next().filter(|b| b == &b's')?;
                    bytes.next().filter(|b| b == &b'=')?;
                    charset = Some(String::from_utf8(bytes.collect::<Vec<u8>>()).ok()?);
                    break
                }
                _ => return None
            }
        }

        return Some(FileStreamMode {
            flags,
            input_mode,
            no_cancellation,
            cloexec,
            read_mmap,
            exclusive_create,
            charset,
        })
    }
}

pub struct FileObject {
    pub source: FileStreamSource,
    pub buffer: FileStreamBuffer,
    pub buffer_index: usize,
    pub read_end: usize,
    pub access_mode: FileAccessMode,
    pub buffering_mode: FileBufferMode,
    pub err: bool,
    pub eof: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileBufferMode {
    Unbuffered,
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

pub struct FileStreamCreateEvent {
    source: FileStreamSource,
    mode: FileStreamMode,
    file_ptr: Option<FilePtr>,
}

impl FileStreamCreateEvent {
    #[inline]
    pub fn new(source: FileStreamSource, mode: FileStreamMode, file_ptr: Option<FilePtr>) -> Self {
        Self { source, mode, file_ptr }
    }
}

impl Event for FileStreamCreateEvent {
    type Success = FilePtr;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let source = mem::replace(&mut self.source, FileStreamSource::Descriptor(-1));

        let file_ptr = match self.file_ptr {
            Some(p) => p,
            None => FilePtr::from_raw(unsafe { crate::unique_mem_create().cast::<libc::FILE>() }).unwrap(),
        };

        state.local.file_objs.insert(file_ptr, FileObject {
            source,
            buffer: FileStreamBuffer::Internal(Box::new([0u8; libc::BUFSIZ as usize])),
            buffer_index: 0,
            read_end: 0,
            access_mode: if self.mode.flags.contains(FileOpenFlags::READWRITE) {
                FileAccessMode::ReadWrite
            } else if self.mode.flags.contains(FileOpenFlags::WRITEONLY) {
                FileAccessMode::WriteOnly
            } else {
                FileAccessMode::ReadOnly
            },
            buffering_mode: FileBufferMode::Block,
            eof: false,
            err: false,
        });

        Outcome::Success(file_ptr)
    }
}

pub struct FileStreamCloseEvent<'a> {
    stream: &'a FilePtr,
}

impl<'a> FileStreamCloseEvent<'a> {
    #[inline]
    pub fn new(stream: &'a FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for FileStreamCloseEvent<'_> {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match state.local.file_objs.remove(self.stream) {
            Some(obj) => {
                if let FileStreamSource::Descriptor(fd) = obj.source {
                    state.local.fds.remove(&Descriptor::from_raw_fd(fd));
                }
                Outcome::Success(())
            },
            None => panic!("UB: unrecognized pointer passed to `fclose()`"),
        }
    }
}

pub enum FileStreamFlushState<'a> {
    Start,
    RunActions(Vec<FlushAction<'a>>),
    Invalid,
}

pub struct FlushAction<'a> {
    ptr: FilePtr,
    event: DescriptorWriteEvent<'a>,
}

pub struct FileStreamFlushEvent<'a> {
    stream: Option<FilePtr>,
    state: FileStreamFlushState<'a>,
}

impl FileStreamFlushEvent<'_> {
    #[inline]
    pub fn new(stream: Option<FilePtr>) -> Self {
        Self {
            stream,
            state: FileStreamFlushState::Start,
        }
    }
}
/*
impl Event for FileStreamFlushEvent<'_> {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: for now, this is a No-op as buffering isn't implemented

        let flush_state = mem::replace(&mut self.state, FileStreamFlushState::Invalid);

        match (flush_state, &self.stream) {
            (FileStreamFlushState::Start, Some(file_ptr)) => match state.local.file_objs.get_mut(file_ptr) {
                Some(obj) => {
                    if let FileAccessMode::ReadOnly = obj.access_mode {
                        log::error!("fflush() called on read-only FILE* stream (undefined behavior)");
                    }

                    if obj.eof || obj.err {
                        return Outcome::Error(Errno::SUCCESS)
                    }

                    // Nothing to flush if buffer_index is at 0
                    if obj.buffer_index == 0 {
                        return Outcome::Success(())
                    }

                    let data = match &obj.buffer {
                        FileStreamBuffer::Internal(buf) => &buf.as_ref()[..obj.buffer_index],
                        FileStreamBuffer::Slice(buf) => unsafe { &buf.as_ref()[..obj.buffer_index] },
                        FileStreamBuffer::None => unreachable!(),
                    };

                    match &mut obj.source {
                        FileStreamSource::Descriptor(fd) => {
                            let desc = Descriptor::from_raw_fd(*fd);

                            let events = vec![
                                FlushAction {
                                    ptr: *file_ptr,
                                    event: DescriptorWriteEvent::new(desc, WriteData::BasicSlice(data)),
                                }
                            ];

                            self.state = FileStreamFlushState::RunActions(events);
                        }
                        FileStreamSource::Slice(cell, dst_idx) => {
                            let dst = unsafe { cell.get_mut().as_mut() };
                            let flush_len = cmp::min(data.len(), dst.len() - *dst_idx);

                            dst[*dst_idx..*dst_idx + flush_len].copy_from_slice(&data[..flush_len]);
                            *dst_idx += flush_len;

                            obj.buffer_index = 0;
                            obj.read_end = 0;

                            if flush_len < data.len() {
                                obj.eof = true;
                                return Outcome::Error(Errno::SUCCESS)
                            }
                        }
                        FileStreamSource::Buffer(cell, dst_idx) => {
                            let dst = cell.get_mut();

                            let overwrite_len = cmp::min(data.len(), dst.len() - *dst_idx);

                            dst[*dst_idx..*dst_idx + overwrite_len].copy_from_slice(&data[..overwrite_len]);
                            dst.extend(&data[overwrite_len..]);
                            *dst_idx += data.len();

                            obj.buffer_index = 0;
                            obj.read_end = 0;
                        }
                    }

                }
                None => panic!("UB: invalid file stream pointer flushed"),
            }
            (FileStreamFlushState::Start, None) => {
                // Flush all open file streams
                for stream in state.local.file_objs.values_mut() {
                    let read_slice = match &stream.buffer {
                        FileStreamBuffer::Internal(vec) => &vec.as_ref()[..stream.buffer_index],
                        FileStreamBuffer::Slice(non_null) => {
                            unsafe { &non_null.as_ref()[..stream.buffer_index] }
                        }
                        FileStreamBuffer::None => continue,
                    };

                    let write_slice = match &mut stream.source {
                        FileStreamSource::Descriptor(fd) => {
                            let descriptor = Descriptor::from_raw_fd(*fd);
                            let Some(fd_info) = state.local.fds.get_mut(&descriptor) else {
                                return Outcome::Error(Errno::EBADF)
                            };

                            // TODO: implement
                        }
                        FileStreamSource::Slice(buf, write_idx) => {
                            // TODO: implement
                        },
                        FileStreamSource::Buffer(cell, write_idx) => {
                            // TODO: implement
                        },
                    };

                    /*
                    match &mut stream.buf {
                        FileStreamBuffer::Internal(vec) => vec.clear(),
                        FileStreamBuffer::Slice(_non_null, write_idx) => *write_idx = 0,
                        FileStreamBuffer::None => unreachable!(),
                    }
                    */
                }
            }
            (FileStreamFlushState::RunActions(v), _) => {

            }
            (FileStreamFlushState::Invalid, _) => unreachable!(),
        }

        Outcome::Success(())
    }
}
*/
pub struct FileStreamDescriptorEvent {
    stream: FilePtr,
}

impl FileStreamDescriptorEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for FileStreamDescriptorEvent {
    type Success = RawFd;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match state.local.file_objs.get(&self.stream) {
            Some(obj) => match &obj.source {
                FileStreamSource::Descriptor(fd) => Outcome::Success(*fd),
                _ => Outcome::Error(Errno::EBADF),
            }
            None => panic!("UB: `fileno()` called on invalid stream pointer"),
        }
    }
}

pub enum FileStreamWriteState<'a> {
    Start,
    Descriptor(DescriptorWriteEvent<'a>),
}

pub struct FileStreamWriteEvent<'a> {
    stream: FilePtr,
    buf: &'a IoSlice<'a>,
    chunk_size: usize,
    state: FileStreamWriteState<'a>,
}

impl<'a> FileStreamWriteEvent<'a> {
    #[inline]
    pub fn new(stream: FilePtr, buf: &'a IoSlice<'a>, chunk_size: usize) -> Self {
        Self { stream, buf, chunk_size, state: FileStreamWriteState::Start }
    }
}

impl Event for FileStreamWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            FileStreamWriteState::Start => {
                let local = &mut state.local;

                if self.chunk_size != 1 {
                    // TODO: chunks cannot be partially written; this needs to be implemented
                    log::warn!("`fwrite()` with member size > 1 not fully implemented--partial writes may occur")
                }

                match local.file_objs.get_mut(&self.stream) {
                    Some(obj) => match &mut obj.source {
                        FileStreamSource::Descriptor(fd) => {
                            let desc = Descriptor::from_raw_fd(*fd);
                            self.state = FileStreamWriteState::Descriptor(DescriptorWriteEvent::new(desc, WriteData::BasicVec(slice::from_ref(self.buf))));
                            Outcome::Continue
                        }
                        FileStreamSource::Slice(cell, write_idx) => {
                            let written = cmp::min(cell.get().len() - *write_idx, self.buf.len());
                            let write_buf = unsafe { cell.get_mut().as_mut() };

                            write_buf[*write_idx..*write_idx + written].copy_from_slice(&self.buf[..written]);
                            *write_idx += written;
                            if *write_idx == cell.get().len() {
                                obj.eof = true; // TODO: is this meant to be set here?
                            }
                            Outcome::Success(written)
                        },
                        FileStreamSource::Buffer(cell, write_idx) => {
                            let v = cell.get_mut();

                            let overwrite_len = cmp::min(v.len() - *write_idx, self.buf.len());

                            v[*write_idx..*write_idx + overwrite_len].copy_from_slice(&self.buf[..overwrite_len]);

                            v.extend_from_slice(&self.buf[overwrite_len..]);
                            *write_idx += self.buf.len();

                            Outcome::Success(self.buf.len())
                        }
                    }
                    None => panic!("UB: `fileno()` called on invalid stream pointer"),
                }
            }
            FileStreamWriteState::Descriptor(ev) => {
                let mut res = ev.run(state);
                if let Outcome::Success(u) = &mut res {
                    *u /= self.chunk_size;
                }
                res
            } 
        }
    }
}

pub enum FileStreamReadState<'a> {
    Start(&'a mut IoSliceMut<'a>),
    Descriptor(DescriptorReadEvent<'a>),
    Invalid,
}

pub struct FileStreamReadEvent<'a> {
    stream: FilePtr,
    chunk_size: usize,
    state: FileStreamReadState<'a>,
}

impl<'a> FileStreamReadEvent<'a> {
    #[inline]
    pub fn new(stream: FilePtr, buf: &'a mut IoSliceMut<'a>, chunk_size: usize) -> Self {
        Self { stream, chunk_size, state: FileStreamReadState::Start(buf) }
    }
}

impl Event for FileStreamReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let mut read_state = FileStreamReadState::Invalid;
        mem::swap(&mut read_state, &mut self.state);

        match read_state {
            FileStreamReadState::Start(buf) => {
                let local = &mut state.local;

                if self.chunk_size != 1 {
                    // TODO: chunks cannot be partially written; this needs to be implemented
                    log::warn!("`fread()` with member size > 1 not fully implemented--partial reads may occur")
                }

                match local.file_objs.get_mut(&self.stream) {
                    Some(obj) => match &mut obj.source {
                        FileStreamSource::Descriptor(fd) => {
                            let desc = Descriptor::from_raw_fd(*fd);
                            self.state = FileStreamReadState::Descriptor(DescriptorReadEvent::new(desc, ReadData::Basic(slice::from_mut(buf))));
                            Outcome::Continue
                        }
                        FileStreamSource::Slice(cell, read_idx) => {
                            let read = cmp::min(cell.get().len() - *read_idx, buf.len());
                            let read_buf = unsafe { cell.get().as_ref() };

                            buf[..read].copy_from_slice(&read_buf[*read_idx..*read_idx + read]);
                            *read_idx += read;
                            if *read_idx == cell.get().len() {
                                obj.eof = true; // TODO: is this meant to be set here?
                            }
                            Outcome::Success(read)
                        },
                        FileStreamSource::Buffer(cell, read_idx) => {
                            let read = cmp::min(cell.get_mut().len() - *read_idx, buf.len());
                            let read_buf = cell.get_mut().as_slice();

                            buf[..read].copy_from_slice(&read_buf[*read_idx..*read_idx + read]);
                            *read_idx += read;
                            if *read_idx == cell.get_mut().len() {
                                obj.eof = true; // TODO: is this meant to be set here?
                            }

                            Outcome::Success(read)
                        }
                    }
                    None => panic!("UB: `fileno()` called on invalid stream pointer"),
                }
            }
            FileStreamReadState::Descriptor(mut ev) => {
                // TODO: add divisor once self.chunk_size > 1 implemented
                let res = ev.run(state);
                self.state = FileStreamReadState::Descriptor(ev);
                res
            },
            FileStreamReadState::Invalid => unreachable!(),
        }
    }
}

