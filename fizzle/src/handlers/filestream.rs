use std::cell::Cell;
use std::collections::VecDeque;
use std::ffi::CStr;
use std::os::fd::RawFd;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, ThreadId};
use std::{cmp, iter, mem, slice};

use crate::constants::FIZZLE_STREAM_BUFSIZ;
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome, YieldUntil};
use crate::state::FizzleState;

use super::descriptor::*;
use super::file::{file_length, FileOpenFlags, FileSeekEvent, SeekPosition};

// This starts at 16 because NULL should indicates failure.
// It increments by 16 to avoid any pointer alignment shenanigans.
static NEXT_FILE_PTR: AtomicUsize = AtomicUsize::new(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePtr(NonNull<libc::FILE>);

impl FilePtr {
    pub fn from_raw(value: *mut libc::FILE) -> Option<Self> {
        Some(FilePtr(NonNull::new(value)?))
    }

    pub fn as_raw(&mut self) -> *mut libc::FILE {
        self.0.as_ptr()
    }

    pub fn allocate() -> Self {
        loop {
            let next = NEXT_FILE_PTR.fetch_add(16, Ordering::Relaxed);
            unsafe {
                if next == crate::stdin.addr()
                    || next == crate::stdout.addr()
                    || next == crate::stderr.addr()
                {
                    continue;
                }
            }

            // SAFETY: it is UB to dereference this pointer.
            return Self(NonNull::new(next as *mut libc::FILE).unwrap());
        }
    }
}

#[derive(PartialEq, Eq)]
pub enum PushbackChar {
    Regular(u8),
    Wide(libc::wchar_t),
    None,
}

pub enum FileStreamSource {
    Descriptor(RawFd),
    Slice(Cell<NonNull<[u8]>>),
    Buffer(Cell<Vec<u8>>),
}

pub enum FileStreamBuffer {
    Internal(Box<[u8]>),
    Slice(NonNull<[u8]>),
    None(PushbackChar),
}

#[derive(PartialEq, Eq)]
pub enum LastFileOperation {
    None,
    Reading,
    Writing,
}

#[derive(PartialEq, Eq)]
pub enum FileOrientation {
    /// The file stream uses 8-bit characters.
    Regular,
    /// The file stream uses 16-bit ("wide") characters.
    Wide,
}

pub struct FileStreamMode<'a> {
    pub flags: FileOpenFlags,
    pub input_mode: FileInputMode,
    pub no_cancellation: bool,
    pub cloexec: bool,
    pub read_mmap: bool,
    pub exclusive_create: bool,
    pub charset: Option<&'a CStr>,
}

pub enum FileInputMode {
    Binary,
    Text,
}

impl<'a> FileStreamMode<'a> {
    pub fn from_cstr(mode: &'a CStr) -> Option<Self> {
        let mut bytes = mode.to_bytes().iter().map(|b| *b).enumerate().peekable();
        let mut no_cancellation = false;
        let mut cloexec = false;
        let mut read_mmap = false;
        let mut exclusive_create = false;
        let mut charset = None;
        let mut input_mode = FileInputMode::Text;

        let flags = match bytes.next()? {
            (_, b'r') => {
                let mut flags = None;
                if bytes.peek().map(|&(_, c)| c) == Some(b'+') {
                    bytes.next();
                    flags = Some(FileOpenFlags::READWRITE);
                }

                match bytes.peek() {
                    Some(&(_, b'b')) => {
                        bytes.next();
                        input_mode = FileInputMode::Binary;
                    }
                    Some(&(_, b't')) => {
                        bytes.next();
                        input_mode = FileInputMode::Text;
                    }
                    _ => (),
                }

                if let Some(flags) = flags {
                    flags
                } else if bytes.peek().map(|&(_, c)| c) == Some(b'+') {
                    bytes.next();
                    FileOpenFlags::READWRITE
                } else {
                    FileOpenFlags::empty() // READONLY
                }
            }
            (_, b'w') => {
                let mut flags = None;
                if bytes.peek().map(|&(_, c)| c) == Some(b'+') {
                    bytes.next();
                    flags = Some(FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::TRUNC);
                }

                match bytes.peek() {
                    Some(&(_, b'b')) => {
                        bytes.next();
                        input_mode = FileInputMode::Binary;
                    }
                    Some(&(_, b't')) => {
                        bytes.next();
                        input_mode = FileInputMode::Text;
                    }
                    _ => (),
                }

                if let Some(flags) = flags {
                    flags
                } else if bytes.peek().map(|&(_, c)| c) == Some(b'+') {
                    bytes.next();
                    FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::TRUNC
                } else {
                    FileOpenFlags::WRITEONLY | FileOpenFlags::CREATE | FileOpenFlags::TRUNC
                }
            }
            (_, b'a') => {
                let mut flags = None;
                if bytes.peek().map(|&(_, c)| c) == Some(b'+') {
                    bytes.next();
                    flags = Some(FileOpenFlags::READWRITE | FileOpenFlags::CREATE | FileOpenFlags::APPEND);
                }

                match bytes.peek() {
                    Some(&(_, b'b')) => {
                        bytes.next();
                        input_mode = FileInputMode::Binary;
                    }
                    Some(&(_, b't')) => {
                        bytes.next();
                        input_mode = FileInputMode::Text;
                    }
                    _ => (),
                }

                if let Some(flags) = flags {
                    flags
                } else if bytes.peek().map(|&(_, c)| c) == Some(b'+') {
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
                (_, b'c') if no_cancellation => return None,
                (_, b'c') => no_cancellation = true,
                (_, b'e') if cloexec => return None,
                (_, b'e') => cloexec = true,
                (_, b'm') if read_mmap => return None,
                (_, b'm') => read_mmap = true,
                (_, b'x') if exclusive_create => return None,
                (_, b'x') => exclusive_create = true,
                (_, b',') => {
                    bytes.next().filter(|(_, b)| b == &b'c')?;
                    bytes.next().filter(|(_, b)| b == &b'c')?;
                    bytes.next().filter(|(_, b)| b == &b's')?;
                    bytes.next().filter(|(_, b)| b == &b'=')?;
                    charset = match bytes.peek().map(|&(idx, _)| idx) {
                        Some(idx) => Some(unsafe { CStr::from_ptr(mode.as_ptr().add(idx)) }),
                        None => return None,
                    };
                    break;
                }
                _ => return None,
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
        });
    }
}

pub struct FileObject {
    pub source: FileStreamSource,
    pub buffer: FileStreamBuffer,
    pub read_idx: usize,
    pub rw_split: usize,
    pub write_idx: usize,
    pub access_mode: FileAccessMode,
    pub buffering_mode: FileBufferMode,
    pub last_op: LastFileOperation,
    pub err: bool,
    pub eof: bool,
    pub locking: bool,
    /// The offset of the underlying file.
    pub offset: usize,
    pub orientation: FileOrientation,
    /// Threads awaiting on the file lock.
    ///
    /// The frontmmost member of this queue represents the thread currently holding the file lock.
    pub queued_threads: VecDeque<ThreadId>,
}

impl FileObject {
    pub fn new(
        source: FileStreamSource,
        access_mode: FileAccessMode,
        orientation: FileOrientation,
    ) -> Self {
        // read_idx is set equal to rw_split to indicate there's no data left to be read.
        let (read_idx, rw_split, write_idx) = match access_mode {
            FileAccessMode::ReadWrite => (
                FIZZLE_STREAM_BUFSIZ / 2,
                FIZZLE_STREAM_BUFSIZ / 2,
                FIZZLE_STREAM_BUFSIZ / 2,
            ),
            FileAccessMode::WriteOnly => (0, 0, 0),
            FileAccessMode::ReadOnly => (
                FIZZLE_STREAM_BUFSIZ,
                FIZZLE_STREAM_BUFSIZ,
                FIZZLE_STREAM_BUFSIZ,
            ),
        };

        Self {
            source,
            buffer: FileStreamBuffer::Internal(Box::new([0u8; libc::BUFSIZ as usize])),
            read_idx,
            rw_split,
            write_idx,
            access_mode,
            buffering_mode: FileBufferMode::Block,
            last_op: LastFileOperation::None,
            eof: false,
            err: false,
            locking: true,
            offset: 0, // TODO: this should be seeked to the end of the file for `append()` mode
            orientation,
            queued_threads: VecDeque::new(),
        }
    }

    pub fn write_buflen(&self) -> usize {
        (match &self.buffer {
            FileStreamBuffer::Internal(s) => s.len(),
            FileStreamBuffer::Slice(s) => s.len(),
            FileStreamBuffer::None(_) => 0,
        }) - self.rw_split
    }

    pub fn readbuf_capacity(&self) -> usize {
        self.rw_split
    }
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

pub struct StreamCreateEvent<'a> {
    source: FileStreamSource,
    mode: FileStreamMode<'a>,
    file_ptr: Option<FilePtr>,
}

impl<'a> StreamCreateEvent<'a> {
    #[inline]
    pub fn new(
        source: FileStreamSource,
        mode: FileStreamMode<'a>,
        file_ptr: Option<FilePtr>,
    ) -> Self {
        Self {
            source,
            mode,
            file_ptr,
        }
    }
}

impl Event for StreamCreateEvent<'_> {
    type Success = FilePtr;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if let FileStreamSource::Descriptor(fd) = self.source {
            match state.local.fds.get_mut(&Descriptor::from_raw_fd(fd)) {
                Some(fd_info) => {
                    fd_info.close_on_exec = self.mode.cloexec;
                }
                None => {
                    state.local.fds.insert(
                        Descriptor::from_raw_fd(fd),
                        DescriptorInfo {
                            close_on_exec: self.mode.cloexec,
                            nonblocking: false,
                            is_passthrough: true,
                            resource: FdResource::Opaque, // TODO: file support currently unimplemented
                        },
                    );
                }
            }
        }

        let source = mem::replace(&mut self.source, FileStreamSource::Descriptor(-1));

        // read_idx is set equal to rw_split to indicate there's no data left to be read.
        let (read_idx, rw_split, write_idx, access_mode) =
            if self.mode.flags.contains(FileOpenFlags::READWRITE) {
                (
                    FIZZLE_STREAM_BUFSIZ / 2,
                    FIZZLE_STREAM_BUFSIZ / 2,
                    FIZZLE_STREAM_BUFSIZ / 2,
                    FileAccessMode::ReadWrite,
                )
            } else if self.mode.flags.contains(FileOpenFlags::WRITEONLY) {
                (0, 0, 0, FileAccessMode::WriteOnly)
            } else {
                (
                    FIZZLE_STREAM_BUFSIZ,
                    FIZZLE_STREAM_BUFSIZ,
                    FIZZLE_STREAM_BUFSIZ,
                    FileAccessMode::ReadOnly,
                )
            };

        let orientation = if self.mode.charset.is_some() {
            FileOrientation::Wide
        } else {
            FileOrientation::Regular // TODO: on first operation on stream, wide vs regular is decided
        };

        let new_file_obj = FileObject {
            source,
            buffer: FileStreamBuffer::Internal(Box::new([0u8; libc::BUFSIZ as usize])),
            read_idx,
            rw_split,
            write_idx,
            access_mode,
            buffering_mode: FileBufferMode::Block,
            last_op: LastFileOperation::None,
            eof: false,
            err: false,
            locking: true,
            offset: 0, // TODO: this should be seeked to the end of the file for `append()` mode
            orientation,
            queued_threads: VecDeque::new(),
        };

        match self.file_ptr {
            Some(p) => {
                let Some(file_obj) = state.local.file_objs.get_mut(&p) else {
                    panic!("[UB] unrecognized pointer passed to `freopen()`")
                };

                if let FileStreamSource::Descriptor(_fd) = file_obj.source {
                    log::error!("TODO: implement fd cleanup on frepoen failure");
                }

                *file_obj = new_file_obj;

                Outcome::Success(p)
            }
            None => {
                let file_ptr = FilePtr::allocate();

                state.local.file_objs.insert(file_ptr, new_file_obj);

                Outcome::Success(file_ptr)
            }
        }
    }
}

pub struct StreamCloseEvent<'a> {
    stream: &'a FilePtr,
    state: StreamCloseState,
}

pub enum StreamCloseState {
    Start,
    CloseFd(DescriptorCloseEvent),
}

impl<'a> StreamCloseEvent<'a> {
    #[inline]
    pub fn new(stream: &'a FilePtr) -> Self {
        Self {
            stream,
            state: StreamCloseState::Start,
        }
    }
}

impl Event for StreamCloseEvent<'_> {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &mut self.state {
            StreamCloseState::Start => match state.local.file_objs.remove(self.stream) {
                Some(obj) => {
                    if let FileStreamSource::Descriptor(fd) = obj.source {
                        self.state = StreamCloseState::CloseFd(DescriptorCloseEvent::new(
                            Descriptor::from_raw_fd(fd),
                        ));
                        Outcome::Yield(YieldUntil::Immediate)
                    } else {
                        Outcome::Success(())
                    }
                }
                None => panic!("[UB] unrecognized pointer passed to `fclose()`"),
            },
            StreamCloseState::CloseFd(ev) => ev.run(state),
        }
    }
}

// TODO: implement
// "For input streams associated with seekable files (e.g., disk files, but
// not pipes or terminals), fflush() discards any buffered data  that  has
// been fetched from the underlying file, but has not been consumed by the
// application."

pub enum StreamFlushState {
    Start,
    FlushSingle(FlushAction, FileBufferMode),
    FlushAll(Vec<FlushAction>, FileBufferMode, Option<Errno>),
    Invalid,
}

pub struct FlushAction {
    stream: FilePtr,
    event: StreamWriteEvent<'static>,
}

pub struct StreamFlushEvent {
    stream: Option<FilePtr>,
    unlocked: bool,
    state: StreamFlushState,
}

impl StreamFlushEvent {
    #[inline]
    pub fn new(stream: Option<FilePtr>, unlocked: bool) -> Self {
        Self {
            stream,
            unlocked,
            state: StreamFlushState::Start,
        }
    }
}

impl Event for StreamFlushEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let flush_state = mem::replace(&mut self.state, StreamFlushState::Invalid);

        match (flush_state, self.stream) {
            (StreamFlushState::Start, Some(stream)) => {
                // First, lock the FILE*
                let Some(file_obj) = state.local.file_objs.get_mut(&stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                let old_buffering = file_obj.buffering_mode;
                file_obj.buffering_mode = FileBufferMode::Unbuffered;

                self.state = StreamFlushState::FlushSingle(
                    FlushAction {
                        stream,
                        event: StreamWriteEvent::new(stream, &[], 1, self.unlocked),
                    },
                    old_buffering,
                );

                return Outcome::Yield(YieldUntil::Immediate);
            }
            (
                StreamFlushState::FlushSingle(FlushAction { stream, mut event }, old_buffering),
                _,
            ) => match event.run(state) {
                Outcome::Success(()) => {
                    let Some(file_obj) = state.local.file_objs.get_mut(&stream) else {
                        panic!("unrecognized FILE* pointer")
                    };

                    file_obj.buffering_mode = old_buffering;
                    Outcome::Success(())
                }
                Outcome::Error(_) => {
                    let e = Errno::get_errno();

                    let Some(file_obj) = state.local.file_objs.get_mut(&stream) else {
                        panic!("unrecognized FILE* pointer")
                    };

                    file_obj.buffering_mode = old_buffering;
                    Outcome::Error(e)
                }
                Outcome::Yield(y) => {
                    self.state =
                        StreamFlushState::FlushSingle(FlushAction { stream, event }, old_buffering);
                    Outcome::Yield(y)
                }
                Outcome::RunTask(t, y) => {
                    self.state =
                        StreamFlushState::FlushSingle(FlushAction { stream, event }, old_buffering);
                    Outcome::RunTask(t, y)
                }
            },
            (StreamFlushState::Start, None) => {
                // Flush all
                let streams: Vec<FilePtr> = state.local.file_objs.keys().copied().collect();

                let Some(last_stream) = streams.last() else {
                    // No streams to be flushed
                    return Outcome::Success(());
                };

                let Some(file_obj) = state.local.file_objs.get_mut(last_stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                let old_buffering = file_obj.buffering_mode;
                file_obj.buffering_mode = FileBufferMode::Unbuffered;

                let flush_actions: Vec<_> = streams
                    .into_iter()
                    .map(|stream| FlushAction {
                        stream,
                        event: StreamWriteEvent::new(stream, &[], 1, self.unlocked),
                    })
                    .collect();

                self.state = StreamFlushState::FlushAll(flush_actions, old_buffering, None);
                Outcome::Yield(YieldUntil::Immediate)
            }
            (StreamFlushState::FlushAll(mut actions, mut old_buffering, mut errno), _) => {
                let action = actions.last_mut().unwrap();

                match action.event.run(state) {
                    Outcome::Success(()) => {
                        let Some(file_obj) = state.local.file_objs.get_mut(&action.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        file_obj.buffering_mode = old_buffering;
                        // Move on to next buffer, if applicable
                        actions.pop();

                        let Some(action) = actions.last() else {
                            return if let Some(errno) = errno {
                                Outcome::Error(errno)
                            } else {
                                Outcome::Success(())
                            };
                        };

                        let Some(file_obj) = state.local.file_objs.get_mut(&action.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        old_buffering = file_obj.buffering_mode;
                        file_obj.buffering_mode = FileBufferMode::Unbuffered;

                        self.state = StreamFlushState::FlushAll(actions, old_buffering, errno);
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Error(_) => {
                        errno.get_or_insert(Errno::get_errno());

                        let Some(file_obj) = state.local.file_objs.get_mut(&action.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        file_obj.buffering_mode = old_buffering;
                        // Move on to next buffer, if applicable
                        actions.pop();

                        let Some(action) = actions.last() else {
                            return if let Some(errno) = errno {
                                Outcome::Error(errno)
                            } else {
                                Outcome::Success(())
                            };
                        };

                        let Some(file_obj) = state.local.file_objs.get_mut(&action.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        old_buffering = file_obj.buffering_mode;
                        file_obj.buffering_mode = FileBufferMode::Unbuffered;

                        self.state = StreamFlushState::FlushAll(actions, old_buffering, errno);
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Yield(y) => {
                        self.state = StreamFlushState::FlushAll(actions, old_buffering, errno);
                        Outcome::Yield(y)
                    }
                    Outcome::RunTask(t, y) => {
                        self.state = StreamFlushState::FlushAll(actions, old_buffering, errno);
                        Outcome::RunTask(t, y)
                    }
                }
            }
            (StreamFlushState::Invalid, _) => unreachable!(),
        }
    }
}

pub struct StreamTellEvent {
    stream: FilePtr,
    state: StreamTellState,
}

enum StreamTellState {
    Start,
    Finish,
}

impl StreamTellEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self {
            stream,
            state: StreamTellState::Start,
        }
    }
}

// NOTE: seek() must be preceded by flush().
impl Event for StreamTellEvent {
    type Success = usize;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        match &self.state {
            StreamTellState::Start => {
                self.state = StreamTellState::Finish;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            StreamTellState::Finish => {
                let offset = file_obj.offset;

                assert_eq!(
                    file_obj.queued_threads.pop_front(),
                    Some(thread::current().id())
                );
                if let Some(&next_thread) = file_obj.queued_threads.front() {
                    state.mark_thread_ready(next_thread);
                }

                Outcome::Success(offset)
            }
        }
    }
}

pub struct StreamSeekEvent {
    stream: FilePtr,
    position: SeekPosition,
    offset: i64,
    clear_err: bool,
    state: StreamSeekState,
}

enum StreamSeekState {
    Start,
    Locked,
    Seek(FileSeekEvent),
    Unlock(Result<usize, Errno>),
    Invalid,
}

impl StreamSeekEvent {
    #[inline]
    pub fn new(stream: FilePtr, position: SeekPosition, offset: i64, clear_err: bool) -> Self {
        Self {
            stream,
            position,
            offset,
            clear_err,
            state: StreamSeekState::Start,
        }
    }
}

// NOTE: seek() must be preceded by flush().
impl Event for StreamSeekEvent {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let seek_state = mem::replace(&mut self.state, StreamSeekState::Invalid);

        match seek_state {
            StreamSeekState::Start => {
                // First, lock the FILE*
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                self.state = StreamSeekState::Locked;

                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                outcome
            }
            StreamSeekState::Locked => {
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                let curr_offset = file_obj.offset;
                // TODO: fflush() and fwrite() need to properly

                match &mut file_obj.source {
                    FileStreamSource::Buffer(b) => {
                        let b_len = b.get_mut().len();

                        let new_offset = match self.position {
                            SeekPosition::Start => self.offset as usize,
                            SeekPosition::Current => {
                                if self.offset < 0 {
                                    match (curr_offset as i64).checked_add(self.offset) {
                                        None | Some(..0) => {
                                            self.state =
                                                StreamSeekState::Unlock(Err(Errno::EINVAL));
                                            return Outcome::Yield(YieldUntil::Immediate);
                                        }
                                        Some(new_offset @ 0..) => new_offset as usize,
                                    }
                                } else {
                                    match (curr_offset as i64).checked_add(self.offset) {
                                        None => {
                                            self.state =
                                                StreamSeekState::Unlock(Err(Errno::EOVERFLOW));
                                            return Outcome::Yield(YieldUntil::Immediate);
                                        }
                                        Some(new_offset) => new_offset as usize,
                                    }
                                }
                            }
                            SeekPosition::End => match (b_len as i64).checked_add(self.offset) {
                                None => {
                                    self.state = StreamSeekState::Unlock(Err(Errno::EOVERFLOW));
                                    return Outcome::Yield(YieldUntil::Immediate);
                                }
                                Some(new_offset) => new_offset as usize,
                            },
                        };

                        if new_offset > b_len {
                            b.get_mut().extend(iter::repeat(0).take(b_len));
                        }

                        file_obj.offset = new_offset;
                        self.state = StreamSeekState::Unlock(Ok(new_offset));
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    FileStreamSource::Slice(s) => {
                        let slice = unsafe { s.get_mut().as_mut() };
                        let slice_len = slice.len() as i64;

                        let new_offset = match self.position {
                            SeekPosition::Start => self.offset as usize,
                            SeekPosition::Current => {
                                if self.offset < 0 {
                                    match (curr_offset as i64).checked_add(self.offset) {
                                        None | Some(..0) => {
                                            self.state =
                                                StreamSeekState::Unlock(Err(Errno::EINVAL));
                                            return Outcome::Yield(YieldUntil::Immediate);
                                        }
                                        Some(new_offset @ 0..) => new_offset as usize,
                                    }
                                } else {
                                    match (curr_offset as i64).checked_add(self.offset) {
                                        None => {
                                            self.state =
                                                StreamSeekState::Unlock(Err(Errno::EOVERFLOW));
                                            return Outcome::Yield(YieldUntil::Immediate);
                                        }
                                        Some(new_offset) => new_offset as usize,
                                    }
                                }
                            }
                            SeekPosition::End => match slice_len.checked_add(self.offset) {
                                None => {
                                    self.state = StreamSeekState::Unlock(Err(Errno::EOVERFLOW));
                                    return Outcome::Yield(YieldUntil::Immediate);
                                }
                                Some(new_offset) if new_offset > slice_len => {
                                    self.state = StreamSeekState::Unlock(Err(Errno::EINVAL));
                                    return Outcome::Yield(YieldUntil::Immediate);
                                }
                                Some(new_offset) => new_offset as usize,
                            },
                        };

                        file_obj.offset = new_offset;
                        self.state = StreamSeekState::Unlock(Ok(new_offset));
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    FileStreamSource::Descriptor(fd) => {
                        let fd = *fd;
                        match file_length(state, fd) {
                            Ok(file_len) => {
                                let new_offset = match self.position {
                                    SeekPosition::Start => self.offset as usize,
                                    SeekPosition::Current => {
                                        if self.offset < 0 {
                                            match (curr_offset as i64).checked_add(self.offset) {
                                                None | Some(..0) => {
                                                    self.state =
                                                        StreamSeekState::Unlock(Err(Errno::EINVAL));
                                                    return Outcome::Yield(YieldUntil::Immediate);
                                                }
                                                Some(new_offset @ 0..) => new_offset as usize,
                                            }
                                        } else {
                                            match (curr_offset as i64).checked_add(self.offset) {
                                                None => {
                                                    self.state = StreamSeekState::Unlock(Err(
                                                        Errno::EOVERFLOW,
                                                    ));
                                                    return Outcome::Yield(YieldUntil::Immediate);
                                                }
                                                Some(new_offset) => new_offset as usize,
                                            }
                                        }
                                    }
                                    SeekPosition::End => match (file_len as i64)
                                        .checked_add(self.offset)
                                    {
                                        None => {
                                            self.state =
                                                StreamSeekState::Unlock(Err(Errno::EOVERFLOW));
                                            return Outcome::Yield(YieldUntil::Immediate);
                                        }
                                        Some(new_offset) if (new_offset as usize) > file_len => {
                                            self.state =
                                                StreamSeekState::Unlock(Err(Errno::EINVAL));
                                            return Outcome::Yield(YieldUntil::Immediate);
                                        }
                                        Some(new_offset) => new_offset as usize,
                                    },
                                };

                                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream)
                                else {
                                    panic!("unrecognized FILE* pointer")
                                };
                                file_obj.offset = new_offset;

                                self.state = StreamSeekState::Seek(FileSeekEvent::new(
                                    Descriptor::from_raw_fd(fd),
                                    SeekPosition::Start,
                                    new_offset.try_into().unwrap(),
                                ));
                                Outcome::Yield(YieldUntil::Immediate)
                            }
                            Err(e) => {
                                self.state = StreamSeekState::Unlock(Err(e));
                                return Outcome::Yield(YieldUntil::Immediate);
                            }
                        }
                    }
                }
            }
            StreamSeekState::Seek(mut ev) => match ev.run(state) {
                Outcome::Success(res) => {
                    self.state = StreamSeekState::Unlock(Ok(res));
                    Outcome::Yield(YieldUntil::Immediate)
                }
                Outcome::Error(e) => {
                    self.state = StreamSeekState::Unlock(Err(e));
                    Outcome::Yield(YieldUntil::Immediate)
                }
                Outcome::RunTask(t, y) => {
                    self.state = StreamSeekState::Seek(ev);
                    Outcome::RunTask(t, y)
                }
                Outcome::Yield(y) => {
                    self.state = StreamSeekState::Seek(ev);
                    Outcome::Yield(y)
                }
            },
            StreamSeekState::Unlock(res) => {
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                if res.is_ok() {
                    file_obj.eof = false;
                    if self.clear_err {
                        file_obj.err = false;
                    }
                }

                assert_eq!(
                    file_obj.queued_threads.pop_front(),
                    Some(thread::current().id())
                );
                if let Some(&next_thread) = file_obj.queued_threads.front() {
                    state.mark_thread_ready(next_thread);
                }

                match res {
                    Ok(offset) => Outcome::Success(offset),
                    Err(e) => Outcome::Error(e),
                }
            }
            StreamSeekState::Invalid => unreachable!(),
        }
    }
}

pub struct StreamDescriptorEvent {
    stream: FilePtr,
    unlocked: bool,
    state: StreamDescriptorState,
}

enum StreamDescriptorState {
    Start,
    Finish,
}

impl StreamDescriptorEvent {
    #[inline]
    pub fn new(stream: FilePtr, unlocked: bool) -> Self {
        Self {
            stream,
            unlocked,
            state: StreamDescriptorState::Start,
        }
    }
}

impl Event for StreamDescriptorEvent {
    type Success = RawFd;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        match &self.state {
            StreamDescriptorState::Start if !self.unlocked => {
                self.state = StreamDescriptorState::Finish;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            _ => {
                let outcome = match &file_obj.source {
                    FileStreamSource::Descriptor(fd) => Outcome::Success(*fd),
                    _ => Outcome::Error(Errno::EBADF),
                };

                if !self.unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                outcome
            }
        }
    }
}

pub enum StreamWriteState<'a> {
    Start,
    WriteToBuffer,
    WriteToDescriptor {
        ev: DescriptorWriteEvent<'a>,
        scratch_ptr: *mut [u8; FIZZLE_STREAM_BUFSIZ],
        scratch_len: usize,
    },
    Finish(Option<Errno>),
    Invalid,
}

pub struct StreamWriteEvent<'a> {
    stream: FilePtr,
    in_buf: &'a [u8],
    chunk_size: usize,
    written: usize,
    unlocked: bool,
    state: StreamWriteState<'a>,
}

impl<'a> StreamWriteEvent<'a> {
    #[inline]
    pub fn new(stream: FilePtr, buf: &'a [u8], chunk_size: usize, unlocked: bool) -> Self {
        Self {
            stream,
            in_buf: buf,
            chunk_size,
            written: 0,
            unlocked,
            state: StreamWriteState::Start,
        }
    }
}

// TODO: Read/Write permissions are not enforced in this implementation yet
impl Event for StreamWriteEvent<'_> {
    type Success = ();
    type Error = usize;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let mut write_state = StreamWriteState::Invalid;
        mem::swap(&mut write_state, &mut self.state);

        match (write_state, self.unlocked) {
            (StreamWriteState::Start, false) => {
                // First, lock the FILE*
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                self.state = StreamWriteState::WriteToBuffer;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                outcome
            }
            (StreamWriteState::Start, true) | (StreamWriteState::WriteToBuffer, _) => {
                let scratch = state.local.get_scratch();

                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                let rw_split = file_obj.rw_split;
                let buf_write_idx = file_obj.write_idx;
                let buffer = match &mut file_obj.buffer {
                    FileStreamBuffer::Internal(filebuf) => &mut filebuf[buf_write_idx..],
                    FileStreamBuffer::Slice(filebuf_ptr) => unsafe {
                        &mut filebuf_ptr.as_mut()[buf_write_idx..]
                    },
                    FileStreamBuffer::None(_) => &mut [],
                };

                let should_flush = match file_obj.buffering_mode {
                    FileBufferMode::Unbuffered => true,
                    FileBufferMode::Line => {
                        buffer.len() < self.in_buf.len() || self.in_buf.contains(&b'\n')
                    }
                    FileBufferMode::Block => buffer.len() < self.in_buf.len(),
                };

                file_obj.last_op = LastFileOperation::Writing;

                // Now write directly to the buffer and return if possible
                if !should_flush {
                    let written = cmp::min(buffer.len(), self.in_buf.len());
                    buffer[..written].copy_from_slice(self.in_buf);
                    file_obj.write_idx += written;
                    self.written += written;

                    state.local.put_scratch(scratch);
                    self.state = StreamWriteState::Finish(None);
                    return Outcome::Yield(YieldUntil::Immediate);
                } else {
                    // BUG: when a newline is encountered, *everything* is flushed. Should this
                    // be changed to only flush up to the last newline?

                    if buf_write_idx == rw_split {
                        self.state = StreamWriteState::Finish(None);
                        return Outcome::Yield(YieldUntil::Immediate);
                    }

                    let buffer = match &mut file_obj.buffer {
                        FileStreamBuffer::Internal(filebuf) => {
                            &mut filebuf[rw_split..buf_write_idx]
                        }
                        FileStreamBuffer::Slice(filebuf_ptr) => unsafe {
                            &mut filebuf_ptr.as_mut()[rw_split..buf_write_idx]
                        },
                        FileStreamBuffer::None(pushback) => {
                            unimplemented!()
                        }
                    };

                    match &mut file_obj.source {
                        FileStreamSource::Descriptor(fd) => {
                            let scratch_ptr = Box::into_raw(scratch);
                            let scratch_len = cmp::min(FIZZLE_STREAM_BUFSIZ, buffer.len());

                            let scratch_buf = unsafe {
                                slice::from_raw_parts_mut(scratch_ptr.cast::<u8>(), scratch_len)
                            };

                            scratch_buf[..scratch_len].copy_from_slice(&buffer[..scratch_len]);

                            self.state = StreamWriteState::WriteToDescriptor {
                                ev: DescriptorWriteEvent::new(
                                    Descriptor::from_raw_fd(*fd),
                                    WriteData::BasicSlice(scratch_buf),
                                ),
                                scratch_ptr,
                                scratch_len,
                            };
                        }
                        FileStreamSource::Slice(cell) => {
                            let offset = file_obj.offset;
                            let dst = unsafe { &mut cell.get_mut().as_mut()[offset..] };

                            let buffer_len = cmp::min(buffer.len(), dst.len());
                            dst[..buffer_len].copy_from_slice(buffer);

                            if buffer_len < buffer.len() {
                                // Shift remainder of buffer back
                                for i in 0..buffer.len() - buffer_len {
                                    buffer[i] = buffer[buffer_len + i];
                                }

                                state.local.put_scratch(scratch);

                                self.state = StreamWriteState::Finish(Some(Errno::SUCCESS));
                                return Outcome::Yield(YieldUntil::Immediate);
                            }

                            let copy_len = cmp::min(self.in_buf.len(), dst.len() - buffer_len);
                            dst[buffer_len..buffer_len + copy_len].copy_from_slice(buffer);

                            file_obj.write_idx -= copy_len;
                            self.written += copy_len;

                            state.local.put_scratch(scratch);

                            if copy_len == self.in_buf.len() {
                                self.state = StreamWriteState::Finish(None);
                            } else {
                                self.state = StreamWriteState::Finish(Some(Errno::SUCCESS));
                            }
                        }
                        FileStreamSource::Buffer(cell) => {
                            let offset = file_obj.offset;
                            let dst = &mut cell.get_mut().as_mut_slice()[offset..];

                            let buffer_len = cmp::min(buffer.len(), dst.len());
                            dst[..buffer_len].copy_from_slice(buffer);
                            file_obj.write_idx -= buffer_len;

                            if buffer_len < buffer.len() {
                                // Shift remainder of buffer back
                                for i in 0..buffer.len() - buffer_len {
                                    buffer[i] = buffer[buffer_len + i];
                                }

                                state.local.put_scratch(scratch);

                                self.state = StreamWriteState::Finish(Some(Errno::SUCCESS));
                                return Outcome::Yield(YieldUntil::Immediate);
                            }

                            let copy_len = cmp::min(self.in_buf.len(), dst.len() - buffer_len);
                            dst[buffer_len..buffer_len + copy_len].copy_from_slice(buffer);

                            file_obj.write_idx += copy_len;
                            self.written += copy_len;

                            state.local.put_scratch(scratch);

                            if copy_len == self.in_buf.len() {
                                self.state = StreamWriteState::Finish(None);
                            } else {
                                self.state = StreamWriteState::Finish(Some(Errno::SUCCESS));
                            }
                        }
                    }
                }

                Outcome::Yield(YieldUntil::Immediate)
            }
            (
                StreamWriteState::WriteToDescriptor {
                    mut ev,
                    scratch_ptr,
                    scratch_len,
                },
                _,
            ) => {
                let fd = ev.fd;
                match ev.run(state) {
                    Outcome::Success(0) => {
                        drop(ev);

                        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        file_obj.err = true;
                        let buffer_len = file_obj.write_idx - file_obj.rw_split;

                        if self.written > 0 && self.written < buffer_len {
                            // A partial write from the output buffer occurred, but then the
                            // underlying destination returned 0. We need to shift the remaining
                            // bytes in the buffer that weren't written.

                            let rw_split = file_obj.rw_split;
                            let buf_write_idx = file_obj.write_idx;

                            let buffer = match &mut file_obj.buffer {
                                FileStreamBuffer::Internal(filebuf) => {
                                    &mut filebuf[rw_split..buf_write_idx]
                                }
                                FileStreamBuffer::Slice(filebuf_ptr) => unsafe {
                                    &mut filebuf_ptr.as_mut()[rw_split..buf_write_idx]
                                },
                                FileStreamBuffer::None(_) => &mut [],
                            };

                            for i in 0..buffer_len - self.written {
                                buffer[i] = buffer[self.written + i];
                            }

                            file_obj.write_idx = rw_split + (buffer_len - self.written);
                        }

                        state
                            .local
                            .put_scratch(unsafe { Box::from_raw(scratch_ptr) });
                        self.state = StreamWriteState::Finish(Some(Errno::EPIPE));
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Success(written) => {
                        drop(ev);

                        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        self.written += written;
                        if written < scratch_len {
                            let new_buf: &[u8] = unsafe {
                                slice::from_raw_parts(
                                    scratch_ptr.cast_const().cast::<u8>().add(self.written),
                                    scratch_len - written,
                                )
                            };

                            self.state = StreamWriteState::WriteToDescriptor {
                                ev: DescriptorWriteEvent::new(fd, WriteData::BasicSlice(new_buf)),
                                scratch_ptr,
                                scratch_len: new_buf.len(),
                            };

                            return Outcome::Yield(YieldUntil::Immediate);
                        } else {
                            // Mark buffer as cleared
                            file_obj.write_idx = file_obj.rw_split;
                        }

                        // The scratch buffer has been fully written; what remains to be written
                        // is the out_buf.

                        // TODO: should line-buffered and block-buffered write modulo remainder?

                        if scratch_len == 0 {
                            self.in_buf = &self.in_buf[written..];
                        }

                        if self.in_buf.is_empty() {
                            self.state = StreamWriteState::Finish(None);
                        } else {
                            self.state = StreamWriteState::WriteToDescriptor {
                                ev: DescriptorWriteEvent::new(
                                    fd,
                                    WriteData::BasicSlice(self.in_buf),
                                ),
                                scratch_ptr,
                                scratch_len: 0,
                            };
                        }
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Error(e) => {
                        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        file_obj.err = true;
                        let buffer_len = file_obj.write_idx - file_obj.rw_split;

                        if self.written > 0 && self.written < buffer_len {
                            // A partial write from the output buffer occurred, but then the
                            // underlying destination returned 0. We need to shift the remaining
                            // bytes in the buffer that weren't written.

                            let rw_split = file_obj.rw_split;
                            let buf_write_idx = file_obj.write_idx;

                            let buffer = match &mut file_obj.buffer {
                                FileStreamBuffer::Internal(filebuf) => {
                                    &mut filebuf[rw_split..buf_write_idx]
                                }
                                FileStreamBuffer::Slice(filebuf_ptr) => unsafe {
                                    &mut filebuf_ptr.as_mut()[rw_split..buf_write_idx]
                                },
                                FileStreamBuffer::None(_) => &mut [],
                            };

                            for i in 0..buffer_len - self.written {
                                buffer[i] = buffer[self.written + i];
                            }

                            file_obj.write_idx = file_obj.rw_split + (buffer_len - self.written);
                        }

                        state
                            .local
                            .put_scratch(unsafe { Box::from_raw(scratch_ptr) });
                        self.state = StreamWriteState::Finish(Some(e));
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::RunTask(t, y) => {
                        self.state = StreamWriteState::WriteToDescriptor {
                            ev,
                            scratch_ptr,
                            scratch_len,
                        };
                        Outcome::RunTask(t, y)
                    }
                    Outcome::Yield(y) => {
                        self.state = StreamWriteState::WriteToDescriptor {
                            ev,
                            scratch_ptr,
                            scratch_len,
                        };
                        Outcome::Yield(y)
                    }
                }
            }
            (StreamWriteState::Finish(errno), unlocked) => {
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                file_obj.offset += self.written;

                if !unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                if let Some(errno) = errno {
                    errno.set_errno();
                    Outcome::Error(self.written / self.chunk_size)
                } else {
                    Outcome::Success(())
                }
            }
            (StreamWriteState::Invalid, _) => unreachable!(),
        }
    }
}

fn find_first(buf: &[u8], value: u8) -> Option<usize> {
    for i in 0..buf.len() {
        if buf[i] == value {
            return Some(i);
        }
    }

    None
}

fn _find_last(buf: &[u8], value: u8) -> Option<usize> {
    for i in (0..buf.len()).rev() {
        if buf[i] == value {
            return Some(i);
        }
    }

    None
}

pub enum StreamReadState<'a> {
    Start(&'a mut [u8]),
    ReadFromBuffer(&'a mut [u8]),
    ReadFromDescriptor(
        DescriptorReadEvent<'a>,
        &'a mut [u8],
        *mut [u8; FIZZLE_STREAM_BUFSIZ],
    ),
    Finish(Option<Errno>),
    Invalid,
}

// `stop_at_newline` is necessary for `ftegs` behavior. The length returned
// by Outcome::Success will always be equal to the input length * chunk_size
// unless stop_at_newline is true and a newline is encountered.
pub struct StreamReadEvent<'a> {
    stream: FilePtr,
    chunk_size: usize,
    unlocked: bool,
    stop_at_newline: bool,
    bytes_read: usize,
    state: StreamReadState<'a>,
}

impl<'a> StreamReadEvent<'a> {
    #[inline]
    pub fn new(
        stream: FilePtr,
        buf: &'a mut [u8],
        chunk_size: usize,
        unlocked: bool,
        stop_at_newline: bool,
    ) -> Self {
        Self {
            stream,
            chunk_size,
            unlocked,
            stop_at_newline,
            bytes_read: 0,
            state: StreamReadState::Start(buf),
        }
    }
}

impl Event for StreamReadEvent<'_> {
    type Success = usize;
    type Error = usize;

    // BUG: "The file position indicator for the stream is advanced by the number of bytes suc
    // cessfully read or written." Our implementation may not adhere to this on partial reads.

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let mut read_state = StreamReadState::Invalid;
        mem::swap(&mut read_state, &mut self.state);

        match (read_state, self.unlocked) {
            (StreamReadState::Start(buf), false) => {
                // First, lock the FILE*
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                self.state = StreamReadState::ReadFromBuffer(buf);
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            (StreamReadState::Start(mut out), true)
            | (StreamReadState::ReadFromBuffer(mut out), _) => {
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                file_obj.last_op = LastFileOperation::Reading;
                if out.is_empty() {
                    self.state = StreamReadState::Finish(None);
                    return Outcome::Yield(YieldUntil::Immediate);
                }

                match &mut file_obj.buffer {
                    FileStreamBuffer::Internal(filebuf) => {
                        let buf = &filebuf[file_obj.read_idx..file_obj.rw_split];

                        if self.stop_at_newline {
                            if let Some(newline_idx) = find_first(buf, b'\n') {
                                let end = cmp::min(out.len(), newline_idx + 1);
                                out = &mut out[..end];
                            }
                        }

                        let read = cmp::min(buf.len(), out.len());
                        out[..read].copy_from_slice(&buf[..read]);
                        file_obj.read_idx += read;
                        self.bytes_read += read;
                    }
                    FileStreamBuffer::Slice(filebuf_ptr) => {
                        let buf =
                            unsafe { &filebuf_ptr.as_ref()[file_obj.read_idx..file_obj.rw_split] };

                        if self.stop_at_newline {
                            if let Some(newline_idx) = find_first(buf, b'\n') {
                                let end = cmp::min(out.len(), newline_idx + 1);
                                out = &mut out[..end];
                            }
                        }

                        let read = cmp::min(buf.len(), out.len());
                        out[..read].copy_from_slice(&buf[..read]);
                        file_obj.read_idx += read;
                        self.bytes_read += read;
                    }
                    FileStreamBuffer::None(pushback) => match pushback {
                        PushbackChar::Regular(c) => {
                            if self.stop_at_newline && *c == b'\n' {
                                out = &mut out[..1];
                            }

                            out[0] = *c;
                            *pushback = PushbackChar::None;
                            self.bytes_read += 1;
                        }
                        PushbackChar::Wide(_wc) => unimplemented!(),
                        PushbackChar::None => (),
                    },
                }

                if self.bytes_read == out.len() {
                    self.state = StreamReadState::Finish(None);
                    return Outcome::Yield(YieldUntil::Immediate);
                }
                assert_eq!(file_obj.read_idx, file_obj.readbuf_capacity());

                let mut out = &mut out[self.bytes_read..];

                let readbuf_cap = file_obj.readbuf_capacity();
                let offset = file_obj.offset;

                // Read bytes from the underlying source into the FILE* buffer/destination
                let source = match &mut file_obj.source {
                    FileStreamSource::Descriptor(fd) => {
                        let desc = Descriptor::from_raw_fd(*fd);
                        // If the read is meant to stop at newlines, it needs to fit within the readbuf's capacity
                        // (plus 1 character for the newline that is written out).
                        let mut scratch_len =
                            cmp::min(FIZZLE_STREAM_BUFSIZ, file_obj.readbuf_capacity());
                        if scratch_len == 0 {
                            scratch_len = if self.stop_at_newline {
                                1
                            } else {
                                cmp::min(FIZZLE_STREAM_BUFSIZ, out.len())
                            };
                        }

                        let scratch_ptr = Box::into_raw(state.local.get_scratch());

                        // We need a slice that:
                        // a) lives long enough to receive the results of the ReadFromDescriptor
                        // state (which may return, making anything in our global state unusable)
                        // b) can be reused despite the &mut [u8] slice being consumed by
                        // ReadFromDescriptor (which makes the `out` buffer unusable here).
                        //
                        // To achieve this, we create a reusable thread-local Boxed array and
                        // take it as our slice. This slice's lifetime isn't inferred by the
                        // compiler, so we have to be careful and make sure it doesn't outlive
                        // the box.
                        //
                        // Once ReadFromDescriptor has completed its given round, we re-create the
                        // boxed slice from the pointer passed to that state and give it back to
                        // SCRATCHPAD, thereby enabling reuse of the same allocated buffer across
                        // multiple calls safely.
                        //
                        // Yes, this code is awful. No, there's no better way I could come up with
                        // after hours of thinking and tinkering with various solutions and I think
                        // it will fundamentally require a fundamental change to the Scheduler/Event
                        // architecture upon which Fizzle is build. Hence this duck-tape solution.
                        let scratch_slice: &mut [u8] =
                            unsafe { slice::from_raw_parts_mut(scratch_ptr.cast(), scratch_len) };

                        self.state = StreamReadState::ReadFromDescriptor(
                            DescriptorReadEvent::new(desc, ReadData::BasicSlice(scratch_slice)),
                            out,
                            scratch_ptr,
                        );
                        return Outcome::Yield(YieldUntil::Immediate);
                    }
                    FileStreamSource::Slice(cell) => {
                        let source_buf = &(unsafe { cell.get_mut().as_ref() })[offset..];
                        source_buf
                    }
                    FileStreamSource::Buffer(cell) => {
                        let source_buf = &cell.get_mut().as_slice()[offset..];
                        source_buf
                    }
                };

                if self.stop_at_newline {
                    if let Some(newline_idx) = find_first(source, b'\n') {
                        let end = cmp::min(out.len(), newline_idx + 1);
                        out = &mut out[..end];
                    }
                }

                // First, copy as much data to the output buffer as we can
                let out_len = cmp::min(out.len(), source.len());
                out[..out_len].copy_from_slice(&source[..out_len]);
                let out = &mut out[out_len..];

                self.bytes_read += out_len;
                let source = &source[out_len..];

                // Next, fill the FILE* read buffer with what wouldn't have been read.
                // This emulates reading to the read buffer and then writing to `out`
                // while avoiding the double-copy.

                // Read between [0, readbuf_cap) bytes such that the total amount read
                // is a multiple of `readcap_buf`.
                let readbuf_difference = (readbuf_cap - (out_len % readbuf_cap)) % readbuf_cap;
                let readbuf_len = cmp::min(readbuf_difference, source.len());

                let rw_split = file_obj.rw_split;
                match &mut file_obj.buffer {
                    FileStreamBuffer::Internal(s) => {
                        let readbuf = &mut s.as_mut()[..rw_split];
                        readbuf[rw_split - readbuf_len..rw_split]
                            .copy_from_slice(&source[..readbuf_len]);
                    }
                    FileStreamBuffer::Slice(s) => {
                        let readbuf = &mut (unsafe { s.as_mut() })[..rw_split];
                        readbuf[rw_split - readbuf_len..rw_split]
                            .copy_from_slice(&source[..readbuf_len]);
                    }
                    FileStreamBuffer::None(_) => (),
                }

                file_obj.read_idx -= readbuf_len;

                if out.is_empty() {
                    self.state = StreamReadState::Finish(None);
                } else {
                    file_obj.eof = true;
                    self.state = StreamReadState::Finish(Some(Errno::SUCCESS)); // No error, just eof
                }

                return Outcome::Yield(YieldUntil::Immediate);
            }
            (StreamReadState::ReadFromDescriptor(mut ev, mut out, mut scratch_ptr), _) => {
                match ev.run(state) {
                    Outcome::Success(0) => {
                        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        // TODO: set error somewhere?
                        file_obj.eof = true;
                        state
                            .local
                            .put_scratch(unsafe { Box::from_raw(scratch_ptr) });
                        self.state = StreamReadState::Finish(Some(Errno::SUCCESS));
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::Success(read) => {
                        let fd = ev.fd;
                        // SAFETY: the exclusive reference `ev` holds to `scratch_ptr` needs to be
                        // dropped before we create a new exclusive reference to it.
                        drop(ev);

                        let mut scratch = unsafe { Box::from_raw(scratch_ptr) };
                        let slice_ptr = scratch.as_mut_ptr();
                        scratch_ptr = Box::into_raw(scratch);

                        assert!(read <= FIZZLE_STREAM_BUFSIZ);
                        // SAFETY: `read` does not extend past the end of the `scratch_ptr` buffer.
                        let readbuf: &[u8] =
                            unsafe { slice::from_raw_parts(slice_ptr.cast_const().cast(), read) };

                        if self.stop_at_newline {
                            if let Some(newline_idx) = find_first(readbuf, b'\n') {
                                let out_len = cmp::min(out.len(), newline_idx + 1);
                                out = &mut out[..out_len];
                            }
                        }

                        let out_len = cmp::min(out.len(), read);
                        out[..out_len].copy_from_slice(&readbuf[..out_len]);
                        let out = &mut out[out_len..];
                        let readbuf = &readbuf[out_len..];
                        self.bytes_read += out_len;

                        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        if !readbuf.is_empty() {
                            // Write the remaining contents to the FILE* buffer.

                            match &mut file_obj.buffer {
                                FileStreamBuffer::Internal(filebuf) => {
                                    let read = readbuf.len();
                                    assert!(read < file_obj.read_idx);
                                    let prev_read_idx = file_obj.read_idx;

                                    filebuf[prev_read_idx - read..prev_read_idx]
                                        .copy_from_slice(readbuf);
                                    file_obj.read_idx -= read;
                                }
                                FileStreamBuffer::Slice(filebuf_ptr) => {
                                    let filebuf = unsafe { filebuf_ptr.as_mut() };
                                    let read = readbuf.len();
                                    assert!(read < file_obj.read_idx);
                                    let prev_read_idx = file_obj.read_idx;

                                    filebuf[prev_read_idx - read..prev_read_idx]
                                        .copy_from_slice(readbuf);
                                    file_obj.read_idx -= read;
                                }
                                FileStreamBuffer::None(_) => unreachable!(),
                            }
                        }

                        if out.is_empty() {
                            state
                                .local
                                .put_scratch(unsafe { Box::from_raw(scratch_ptr) });
                            self.state = StreamReadState::Finish(None);
                            Outcome::Yield(YieldUntil::Immediate)
                        } else {
                            let mut scratch_len =
                                cmp::min(FIZZLE_STREAM_BUFSIZ, file_obj.readbuf_capacity());
                            if scratch_len == 0 {
                                scratch_len = if self.stop_at_newline {
                                    1
                                } else {
                                    cmp::min(FIZZLE_STREAM_BUFSIZ, out.len())
                                };
                            }

                            let scratch_slice =
                                unsafe { slice::from_raw_parts_mut(slice_ptr, scratch_len) };
                            self.state = StreamReadState::ReadFromDescriptor(
                                DescriptorReadEvent::new(fd, ReadData::BasicSlice(scratch_slice)),
                                out,
                                scratch_ptr,
                            );
                            Outcome::Yield(YieldUntil::Immediate)
                        }
                    }
                    Outcome::Error(e) => {
                        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                            panic!("unrecognized FILE* pointer")
                        };

                        // TODO: set error somewhere?
                        file_obj.err = true;
                        state
                            .local
                            .put_scratch(unsafe { Box::from_raw(scratch_ptr) });
                        self.state = StreamReadState::Finish(Some(e));
                        Outcome::Yield(YieldUntil::Immediate)
                    }
                    Outcome::RunTask(task, yield_until) => {
                        self.state = StreamReadState::ReadFromDescriptor(ev, out, scratch_ptr);
                        Outcome::RunTask(task, yield_until)
                    }
                    Outcome::Yield(yield_until) => {
                        self.state = StreamReadState::ReadFromDescriptor(ev, out, scratch_ptr);
                        Outcome::Yield(yield_until)
                    }
                }
            }
            (StreamReadState::Finish(errno), unlocked) => {
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                file_obj.offset += self.bytes_read;

                if !unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                if let Some(errno) = errno {
                    errno.set_errno();
                    Outcome::Error(self.bytes_read / self.chunk_size)
                } else {
                    Errno::SUCCESS.set_errno();
                    Outcome::Success(self.bytes_read / self.chunk_size)
                }
            }
            (StreamReadState::Invalid, _) => unreachable!(),
        }
    }
}

pub struct StreamUngetEvent {
    stream: FilePtr,
    character: u8,
    unlocked: bool,
    state: StreamUngetState,
}

enum StreamUngetState {
    Start,
    Finish,
}

impl StreamUngetEvent {
    #[inline]
    pub fn new(stream: FilePtr, character: u8, unlocked: bool) -> Self {
        Self {
            stream,
            character,
            unlocked,
            state: StreamUngetState::Start,
        }
    }
}

impl Event for StreamUngetEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        assert!(file_obj.orientation == FileOrientation::Regular);

        match &self.state {
            StreamUngetState::Start if !self.unlocked => {
                self.state = StreamUngetState::Finish;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            _ => {
                file_obj.last_op = LastFileOperation::Reading; // TODO: is this correct?
                let read_idx = file_obj.read_idx;
                let outcome = match &mut file_obj.buffer {
                    FileStreamBuffer::Internal(_) | FileStreamBuffer::Slice(_) if read_idx == 0 => {
                        Outcome::Error(())
                    }
                    FileStreamBuffer::Internal(s) => {
                        file_obj.read_idx -= 1;
                        s[read_idx - 1] = self.character;
                        file_obj.offset -= 1;
                        file_obj.eof = false;
                        Outcome::Success(())
                    }
                    FileStreamBuffer::Slice(s) => {
                        file_obj.read_idx -= 1;
                        unsafe {
                            s.as_mut()[read_idx - 1] = self.character;
                        }
                        file_obj.offset -= 1;
                        file_obj.eof = false;
                        Outcome::Success(())
                    }
                    FileStreamBuffer::None(pushback) => {
                        if matches!(pushback, PushbackChar::None) {
                            file_obj.offset -= 1;
                            *pushback = PushbackChar::Regular(self.character);
                            file_obj.eof = false;
                            Outcome::Success(())
                        } else {
                            log::warn!("ungetc() called more than once consecutively");
                            file_obj.err = true;
                            Outcome::Error(())
                        }
                    }
                };

                if !self.unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                outcome
            }
        }
    }
}

pub struct StreamErrorEvent {
    stream: FilePtr,
    unlocked: bool,
    state: StreamErrorState,
}

impl StreamErrorEvent {
    #[inline]
    pub fn new(stream: FilePtr, unlocked: bool) -> Self {
        Self {
            stream,
            unlocked,
            state: StreamErrorState::Start,
        }
    }
}

pub enum StreamErrorState {
    Start,
    Finish,
}

impl Event for StreamErrorEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        match &self.state {
            StreamErrorState::Start if !self.unlocked => {
                self.state = StreamErrorState::Finish;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            _ => {
                let err = file_obj.err;

                if !self.unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                Outcome::Success(err)
            }
        }
    }
}

pub struct StreamEofEvent {
    stream: FilePtr,
    unlocked: bool,
    state: StreamEofState,
}

pub enum StreamEofState {
    Start,
    Finish,
}

impl StreamEofEvent {
    #[inline]
    pub fn new(stream: FilePtr, unlocked: bool) -> Self {
        Self {
            stream,
            unlocked,
            state: StreamEofState::Start,
        }
    }
}

impl Event for StreamEofEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        match &self.state {
            StreamEofState::Start if !self.unlocked => {
                self.state = StreamEofState::Finish;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            _ => {
                let eof = file_obj.eof;

                if !self.unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                Outcome::Success(eof)
            }
        }
    }
}

pub struct StreamClearErrorEvent {
    stream: FilePtr,
    unlocked: bool,
    state: StreamClearErrorState,
}

pub enum StreamClearErrorState {
    Start,
    Finish,
}

impl StreamClearErrorEvent {
    #[inline]
    pub fn new(stream: FilePtr, unlocked: bool) -> Self {
        Self {
            stream,
            unlocked,
            state: StreamClearErrorState::Start,
        }
    }
}

impl Event for StreamClearErrorEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        match &self.state {
            StreamClearErrorState::Start if !self.unlocked => {
                self.state = StreamClearErrorState::Finish;
                let outcome = if file_obj.queued_threads.is_empty() {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                };

                file_obj.queued_threads.push_back(thread::current().id());
                return outcome;
            }
            _ => {
                file_obj.eof = false;
                file_obj.err = false;

                if !self.unlocked {
                    assert_eq!(
                        file_obj.queued_threads.pop_front(),
                        Some(thread::current().id())
                    );
                    if let Some(&next_thread) = file_obj.queued_threads.front() {
                        state.mark_thread_ready(next_thread);
                    }
                }

                Outcome::Success(())
            }
        }
    }
}

pub struct StreamBufSizeEvent {
    stream: FilePtr,
}

impl StreamBufSizeEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamBufSizeEvent {
    type Success = usize;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        Outcome::Success(match &file_obj.buffer {
            FileStreamBuffer::Internal(s) => s.len(),
            FileStreamBuffer::Slice(s) => s.len(),
            FileStreamBuffer::None(_pushback) => 0,
        })
    }
}

pub struct StreamSetLockingEvent {
    stream: FilePtr,
    locking: Option<bool>,
}

impl StreamSetLockingEvent {
    #[inline]
    pub fn new(stream: FilePtr, locking: Option<bool>) -> Self {
        Self { stream, locking }
    }
}

impl Event for StreamSetLockingEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        let old_locking = file_obj.locking;
        if let Some(new_locking) = self.locking {
            file_obj.locking = new_locking;
        }

        Outcome::Success(old_locking)
    }
}

pub struct StreamLockEvent {
    stream: FilePtr,
    state: StreamLockState,
}

pub enum StreamLockState {
    Start,
    Finish,
}

impl StreamLockEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self {
            stream,
            state: StreamLockState::Start,
        }
    }
}

impl Event for StreamLockEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &self.state {
            StreamLockState::Start => {
                self.state = StreamLockState::Finish;

                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("unrecognized FILE* pointer")
                };

                let current_id = thread::current().id();
                file_obj.queued_threads.push_back(current_id);

                if file_obj.queued_threads.front() == Some(&current_id) {
                    Outcome::Yield(YieldUntil::Immediate)
                } else {
                    Outcome::Yield(YieldUntil::None)
                }
            }
            StreamLockState::Finish => {
                let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
                    panic!("FILE* pointer destroyed while thread was waiting on it")
                };

                if file_obj.queued_threads.front() != Some(&thread::current().id()) {
                    panic!("internal Fizzle error: FILE* lock operation returned for non-owning thread")
                };

                Outcome::Success(())
            }
        }
    }
}

pub struct StreamTryLockEvent {
    stream: FilePtr,
}

impl StreamTryLockEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamTryLockEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        let current_id = thread::current().id();
        file_obj.queued_threads.push_back(current_id);

        if file_obj.queued_threads.front() == Some(&current_id) {
            Outcome::Success(())
        } else {
            Outcome::Error(())
        }
    }
}

pub struct StreamUnlockEvent {
    stream: FilePtr,
}

impl StreamUnlockEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamUnlockEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        assert_eq!(
            file_obj.queued_threads.pop_front(),
            Some(thread::current().id()),
            "[UB] FILE* unlock operation called by non-owning thread"
        );

        if let Some(&next_thread) = file_obj.queued_threads.front() {
            state.mark_thread_ready(next_thread);
        }

        Outcome::Success(())
    }
}

pub struct StreamPurgeEvent {
    stream: FilePtr,
}

impl StreamPurgeEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamPurgeEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get_mut(&self.stream) else {
            return Outcome::Error(Errno::EBADF);
        };

        match file_obj.queued_threads.front() {
            Some(thread_id) if thread_id != &thread::current().id() => {
                panic!("[UB] FILE* purge() called on locked thread by non-owner")
            }
            _ => (),
        }

        let rw_split = file_obj.rw_split;
        // Clear all buffered read data
        file_obj.read_idx = rw_split;
        // Clear all buffered write data
        file_obj.write_idx = rw_split;

        Outcome::Success(())
    }
}

pub struct StreamWritingEvent {
    stream: FilePtr,
}

impl StreamWritingEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamWritingEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        if file_obj.access_mode == FileAccessMode::WriteOnly
            || file_obj.last_op == LastFileOperation::Writing
        {
            Outcome::Success(true)
        } else {
            Outcome::Success(false)
        }
    }
}

pub struct StreamReadingEvent {
    stream: FilePtr,
}

impl StreamReadingEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamReadingEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        if file_obj.access_mode == FileAccessMode::ReadOnly
            || file_obj.last_op == LastFileOperation::Reading
        {
            Outcome::Success(true)
        } else {
            Outcome::Success(false)
        }
    }
}

pub struct StreamWritableEvent {
    stream: FilePtr,
}

impl StreamWritableEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamWritableEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        Outcome::Success(file_obj.access_mode != FileAccessMode::ReadOnly)
    }
}

pub struct StreamReadableEvent {
    stream: FilePtr,
}

impl StreamReadableEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamReadableEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        Outcome::Success(file_obj.access_mode != FileAccessMode::WriteOnly)
    }
}

pub struct StreamPendingEvent {
    stream: FilePtr,
}

impl StreamPendingEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamPendingEvent {
    type Success = usize;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        let rw_split = file_obj.rw_split;
        let charsize = match file_obj.orientation {
            FileOrientation::Regular => 1,
            FileOrientation::Wide => 2,
        };

        Outcome::Success((file_obj.write_idx - rw_split) / charsize)
    }
}

pub struct StreamLineBufferedEvent {
    stream: FilePtr,
}

impl StreamLineBufferedEvent {
    #[inline]
    pub fn new(stream: FilePtr) -> Self {
        Self { stream }
    }
}

impl Event for StreamLineBufferedEvent {
    type Success = bool;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(file_obj) = state.local.file_objs.get(&self.stream) else {
            panic!("unrecognized FILE* pointer")
        };

        Outcome::Success(file_obj.buffering_mode == FileBufferMode::Line)
    }
}
