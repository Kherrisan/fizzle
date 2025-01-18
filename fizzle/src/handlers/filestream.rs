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
    Internal(Vec<u8>),
    Slice(NonNull<[u8]>, usize),
    None,
}

pub struct FileStreamMode {
    pub flags: FileOpenFlags,
    pub no_cancellation: bool,
    pub cloexec: bool,
    pub read_mmap: bool,
    pub exclusive_create: bool,
    pub charset: Option<String>,
}

impl FileStreamMode {
    pub fn from_cstr(mode: &CStr) -> Option<Self> {
        let mut bytes = mode.to_bytes().iter().map(|b| *b).peekable();
        let mut no_cancellation = false;
        let mut cloexec = false;
        let mut read_mmap = false;
        let mut exclusive_create = false;
        let mut charset = None;

        let flags = match bytes.next()? {
            b'r' => if bytes.peek() == Some(&b'+') {
                bytes.next();
                FileOpenFlags::READWRITE
            } else {
                FileOpenFlags::READONLY
            }
            b'w' => if bytes.peek() == Some(&b'+') {
                bytes.next();
                FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::TRUNC
            } else {
                FileOpenFlags::WRITEONLY | FileOpenFlags::CREATE | FileOpenFlags::TRUNC
            }
            b'a' => if bytes.peek() == Some(&b'+') {
                bytes.next();
                FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::APPEND
            } else {
                FileOpenFlags::WRITEONLY | FileOpenFlags::CREATE | FileOpenFlags::APPEND
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
    pub buf: FileStreamBuffer,
    pub err: bool,
    pub eof: bool,
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
            buf: FileStreamBuffer::Internal(Vec::new()),
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

pub struct FileStreamFlushEvent {
    stream: Option<FilePtr>,
}

impl FileStreamFlushEvent {
    #[inline]
    pub fn new(stream: Option<FilePtr>) -> Self {
        Self { stream }
    }
}

impl Event for FileStreamFlushEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: for now, this is a No-op as buffering isn't implemented

        match &self.stream {
            Some(stream) => match state.local.file_objs.get_mut(stream) {
                Some(obj) => {

                }
                None => panic!("UB: invalid file stream pointer flushed"),
            }
            None => {
                // Flush all open file streams
                for stream in state.local.file_objs.values_mut() {
                    let read_slice = match &stream.buf {
                        FileStreamBuffer::Internal(vec) => vec.as_slice(),
                        FileStreamBuffer::Slice(non_null, write_idx) => {
                            unsafe { &non_null.as_ref()[..*write_idx] }
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
        }

        Outcome::Success(())
    }
}

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
                    unimplemented!("`fwrite()` with member size > 1")
                }

                match local.file_objs.get_mut(&self.stream) {
                    Some(obj) => match &mut obj.source {
                        FileStreamSource::Descriptor(fd) => {
                            let desc = Descriptor::from_raw_fd(*fd);
                            self.state = FileStreamWriteState::Descriptor(DescriptorWriteEvent::new(desc, WriteData::Basic(slice::from_ref(self.buf))));
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
            FileStreamWriteState::Descriptor(ev) => ev.run(state), // TODO: add divisor once self.chunk_size > 1 implemented
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
                    unimplemented!("`fwrite()` with member size > 1")
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

