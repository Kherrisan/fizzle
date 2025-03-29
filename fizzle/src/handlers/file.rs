use std::cell::RefCell;
use std::fmt::Display;
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::rc::Rc;
use std::time::Duration;
use std::{cmp, ptr};

use bitflags::bitflags;
use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;

use crate::backend::{FileBackend, FileFeedback};
use crate::errno::Errno;
use crate::handlers::descriptor::*;
use crate::scheduler::{fizzle_alloc, CreateCowTask, Event, Outcome, YieldUntil};
use crate::state::{CreateCowSource, FizzleState};
use crate::task::Task;
use crate::GlobalRc;

use super::descriptor::{Descriptor, ReadData, WriteData};

pub struct OpenFileInfo {
    pub offset: usize,
    pub file: GlobalRc<FileInfo>,
}

pub struct FileInfo {
    pub path: FilePath<MAX_PATH_LEN>,
    /// The copy-on-write (CoW) shared memory being used to store modifications to the file.
    pub cow: Option<CowId>,
    /// ID of device containing file.
    pub dev_id: libc::dev_t,
    /// Inode number of file.
    pub inode: libc::ino_t,
    /// Permissions mode of file.
    pub mode: AccessMode,
    /// Number of hard links.
    pub nlink: usize,
    /// User ID of owner.
    pub uid: libc::uid_t,
    /// Group ID of owner.
    pub gid: libc::gid_t,
    /*
    /// Total size, in bytes.
    pub size: usize,
    /// Block size for filesystem I/O.
    pub blksize: usize,
    /// Number of 512-byte blocks allocated.
    pub blocks: usize,
    */
    /// Time of last access.
    pub atime: Duration,
    /// Time of creation.
    pub btime: Duration,
    /// Time of last modification.
    pub mtime: Duration,
    /// Time of last status change.
    pub ctime: Duration,
    pub backend: FileBackend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CowId(usize);

impl CowId {
    pub fn first() -> Self {
        Self(0)
    }

    pub fn next(&self) -> Self {
        Self(self.0 + 1)
    }
}

impl From<CowId> for usize {
    fn from(value: CowId) -> Self {
        value.0
    }
}

pub struct CowInfo {
    pub memfd: RawFd,
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
        f.write_fmt(format_args!(
            "{}{}{}{}",
            (self.bits() >> 9) & 7,
            (self.bits() >> 6) & 7,
            (self.bits() >> 3) & 7,
            self.bits() & 7
        ))
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct FileOpenFlags: libc::c_int {
        const APPEND = libc::O_APPEND;
        const ASYNC = libc::O_ASYNC;
        const CLOEXEC = libc::O_CLOEXEC;
        const CREATE = libc::O_CREAT;
        const DIRECT = libc::O_DIRECT;
        const DIRECTORY = libc::O_DIRECTORY;
        const DSYNC = libc::O_DSYNC;
        const EXCLUSIVE = libc::O_EXCL;
        const LARGEFILE = libc::O_LARGEFILE;
        const NOATIME = libc::O_NOATIME;
        const NOCTTY = libc::O_NOCTTY;
        const NOFOLLOW = libc::O_NOFOLLOW;
        const NONBLOCK = libc::O_NONBLOCK;
        const NODELAY = libc::O_NDELAY;
        const PATH = libc::O_PATH;
        const SYNC = libc::O_SYNC;
        const TMPFILE = libc::O_TMPFILE;
        const TRUNC = libc::O_TRUNC;
        const WRITEONLY = libc::O_WRONLY;
        const READWRITE = libc::O_RDWR;
    }
}

pub enum FileOpenLocation {
    Path(FilePath<MAX_PATH_LEN>),
    PathAt(FilePath<MAX_PATH_LEN>, RawFd),
    FileHandle,
}

pub struct FileOpenEvent {
    location: FileOpenLocation,
    flags: FileOpenFlags,
    mode: Option<AccessMode>,
    state: FileOpenState,
}

impl FileOpenEvent {
    #[inline]
    pub fn new(location: FileOpenLocation, flags: FileOpenFlags, mode: Option<AccessMode>) -> Self {
        Self {
            location,
            flags,
            mode,
            state: FileOpenState::Start,
        }
    }
}

pub enum FileOpenState {
    Start,
    CreateCow(CowId),
    Finish,
}

impl Event for FileOpenEvent {
    type Success = RawFd;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let path = match &self.location {
            FileOpenLocation::Path(path) => match path.is_absolute() {
                true => path.clone(),
                false => match state.local.working_directory.clone().concat(path) {
                    Ok(path) => path,
                    Err(_) => return Outcome::Error(Errno::EINVAL),
                },
            },
            FileOpenLocation::PathAt(path, dirfd) => {
                if path.is_absolute() {
                    path.clone()
                } else if *dirfd == libc::AT_FDCWD {
                    let cwd = &state.local.working_directory;
                    let Ok(path) = cwd.clone().concat(&path) else {
                        panic!("filepath too long to concat")
                    };

                    path
                } else {
                    let Some(DescriptorInfo {
                        resource: FdResource::Directory(dir),
                        ..
                    }) = state
                        .local
                        .fds
                        .get(&Descriptor::from_raw_fd(*dirfd))
                        .cloned()
                    else {
                        log::debug!("`openat` called with unrecognized file descriptor");
                        return Outcome::Error(Errno::ENOTDIR);
                    };

                    let Ok(path) = dir.borrow().path.clone().concat(&path) else {
                        panic!("filepath too long to concat")
                    };

                    path
                }
            }

            FileOpenLocation::FileHandle => todo!(),
        };

        match self.state {
            FileOpenState::Start => match state.global.file_paths.get(&path) {
                Some(file_info) => {
                    if self
                        .flags
                        .contains(FileOpenFlags::CREATE | FileOpenFlags::EXCLUSIVE)
                    {
                        Outcome::Error(Errno::EEXIST)
                    } else if let Some(cow_id) = file_info.borrow().cow.clone() {
                        if let Some(cow_info) = state.local.pasture.get(&cow_id) {
                            Outcome::Success(cow_info.memfd)
                        } else {
                            self.state = FileOpenState::CreateCow(cow_id);
                            Outcome::Yield(YieldUntil::Immediate)
                        }
                    } else {
                        // Neither local nor global CoW exist; return actual file
                        let fd = match self.mode {
                            Some(mode) => unsafe {
                                libc::open(path.as_cstr().as_ptr(), self.flags.bits(), mode)
                            },
                            None => unsafe {
                                libc::open(path.as_cstr().as_ptr(), self.flags.bits())
                            },
                        };

                        if fd < 0 {
                            panic!("File listed in files but not found in underlying filesystem--file likely deleted while Fizzle was running")
                        }

                        Outcome::Success(fd)
                    }
                }
                None => {
                    if self.flags.contains(FileOpenFlags::TRUNC)
                        && self
                            .flags
                            .intersects(FileOpenFlags::WRITEONLY | FileOpenFlags::READWRITE)
                    {
                        // The file is immediately truncated, so it is as if it has been wiped
                        return Outcome::RunTask(
                            Task::CreateCow(CreateCowTask(
                                CreateCowSource::New(path, self.mode.unwrap_or(state.local.umask))
                            )),
                            YieldUntil::Reschedule(Duration::ZERO),
                        )
                    }

                    let flag_bits = self
                        .flags
                        .difference(
                            FileOpenFlags::CREATE | FileOpenFlags::EXCLUSIVE | FileOpenFlags::TRUNC,
                        )
                        .bits();
                    let fd = match self.mode {
                        Some(mode) => unsafe {
                            libc::open(path.as_cstr().as_ptr(), flag_bits, mode)
                        },
                        None => unsafe { libc::open(path.as_cstr().as_ptr(), flag_bits) },
                    };

                    if fd >= 0 {
                        if self
                            .flags
                            .contains(FileOpenFlags::CREATE | FileOpenFlags::EXCLUSIVE)
                        {
                            unsafe {
                                libc::close(fd);
                            }
                            return Outcome::Error(Errno::EEXIST);
                        }

                        // TODO: the below need to come from the existing file...
                        // Pull mode, uid and gid from real file here

                        let inode = state.global.next_inode();
                        let mode = self.mode.unwrap_or(state.local.umask);
                        let uid = state.global.uid;
                        let gid = state.global.gid;
                        let current_time = state.global.current_time;

                        let file_info = Rc::new_in(
                            RefCell::new(FileInfo {
                                path: path.clone(),
                                cow: None,
                                dev_id: 0xfe01, // plausible
                                inode,
                                backend: FileBackend::Feedback(FileFeedback {}),
                                mode,
                                nlink: 1, // TODO: implement
                                uid,
                                gid,
                                atime: current_time,
                                btime: current_time,
                                ctime: current_time,
                                mtime: current_time,
                            }),
                            fizzle_alloc(),
                        );
                        if state.global.file_paths.insert(path, file_info).is_err() {
                            panic!("failed to insert to file_paths")
                        }

                        Outcome::Success(fd)
                    } else if self.flags.contains(FileOpenFlags::CREATE) {
                        return Outcome::RunTask(
                            Task::CreateCow(CreateCowTask(
                                CreateCowSource::New(path, self.mode.unwrap_or(state.local.umask))
                            )),
                            YieldUntil::Reschedule(Duration::ZERO),
                        )
                    } else {
                        Outcome::Error(Errno::ENOENT)
                    }
                }
            },
            FileOpenState::CreateCow(cow_id) => {
                self.state = FileOpenState::Finish;
                return Outcome::RunTask(
                    Task::CreateCow(CreateCowTask(CreateCowSource::Existing(cow_id))),
                    YieldUntil::Reschedule(Duration::ZERO),
                )
            }
            FileOpenState::Finish => {
                let file = state.global.file_paths.get(&path).unwrap();
                let cow_id = file.borrow().cow.unwrap();
                let fd = state.local.pasture.get(&cow_id).unwrap().memfd;
                Outcome::Success(fd)
            }
        }
    }
}

pub struct FileReadEvent<'a> {
    fd: Descriptor,
    data: ReadData<'a>,
    cow_created: bool,
}

impl<'a> FileReadEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: ReadData<'a>) -> Self {
        Self {
            fd,
            data,
            cow_created: false,
        }
    }
}

impl Event for FileReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.fd) else {
            return Outcome::Error(Errno::get_errno());
        };

        let FdResource::File(open_file) = &fd_info.resource else {
            unreachable!("non-file fd passed to FileReadEvent")
        };

        let file = open_file.borrow().file.clone();
        let file_ref = file.borrow();

        match &file_ref.backend {
            FileBackend::Passthrough => todo!(),
            FileBackend::Peered(_) => todo!(),
            FileBackend::Feedback(_) => {
                // TODO: edit modify time here
                // file_info.mtime

                let fd = if let Some(cow_id) = file.borrow().cow {
                    let Some(cow_info) = state.local.pasture.get(&cow_id) else {
                        self.cow_created = true;
                        return Outcome::RunTask(
                            Task::CreateCow(CreateCowTask(CreateCowSource::Existing(cow_id))),
                            YieldUntil::Reschedule(Duration::ZERO),
                        )
                    };

                    cow_info.memfd
                } else {
                    self.fd.as_raw_fd()
                };

                if self.cow_created {
                    // Update the file offset to be appropriate
                    let offset = unsafe { libc::lseek(self.fd.as_raw_fd(), 0, libc::SEEK_CUR) };
                    assert!(offset >= 0);

                    open_file.borrow_mut().offset = offset as usize;
                }

                let offset = open_file.borrow().offset;

                match &mut self.data {
                    ReadData::BasicSlice(data) => {
                        let read = unsafe {
                            libc::pread(
                                fd,
                                data.as_mut_ptr().cast(),
                                data.len(),
                                offset as i64,
                            )
                        };
                        if read < 0 {
                            let e = Errno::get_errno();
                            log::warn!(
                                "pread() failed with {} when reading data from file backend",
                                e
                            );
                            return Outcome::Error(e);
                        }

                        open_file.borrow_mut().offset += read as usize;

                        Outcome::Success(read as usize)
                    }
                    ReadData::Iovec(data) => {
                        let read = unsafe {
                            libc::preadv(
                                fd,
                                data.as_mut_ptr().cast::<libc::iovec>(),
                                data.len() as i32,
                                offset as i64,
                            )
                        };
                        if read < 0 {
                            let e = Errno::get_errno();
                            log::warn!(
                                "preadv() failed with {} when reading data from file backend",
                                e
                            );
                            return Outcome::Error(e);
                        }

                        open_file.borrow_mut().offset += read as usize;

                        Outcome::Success(read as usize)
                    }
                    ReadData::File(file_read_data) => {
                        // TODO: handle flags
                        let data = &mut file_read_data.buf;
                        let offset = file_read_data
                            .offset
                            .unwrap_or(open_file.borrow().offset as i64);
                        let read = unsafe {
                            libc::preadv(
                                fd,
                                data.as_mut_ptr().cast::<libc::iovec>(),
                                data.len() as i32,
                                offset as i64,
                            )
                        };
                        if read < 0 {
                            let e = Errno::get_errno();
                            log::warn!(
                                "readv() failed with {} when reading data from file backend",
                                e
                            );
                            return Outcome::Error(e);
                        }

                        open_file.borrow_mut().offset += read as usize;

                        Outcome::Success(read as usize)
                    }
                    ReadData::Socket(_, _) => return Outcome::Error(Errno::ENOTSOCK),
                }
            }
            FileBackend::Plugin(_plugin_endpoint_id) => {
                unimplemented!("file plugins not implemented")
            }
            FileBackend::Sink => return Outcome::Success(0),
            FileBackend::NullSink => match &mut self.data {
                ReadData::BasicSlice(data) => {
                    data.fill(0);
                    Outcome::Success(data.len())
                }
                ReadData::Iovec(data) => {
                    let mut total_read = 0;
                    for s in data.iter_mut() {
                        s.fill(0);
                        total_read += s.len();
                    }

                    Outcome::Success(total_read)
                }
                ReadData::File(data) => {
                    let mut total_read = 0;
                    for s in data.buf.iter_mut() {
                        s.fill(0);
                        total_read += s.len();
                    }

                    Outcome::Success(total_read)
                }
                ReadData::Socket(_, _) => return Outcome::Error(Errno::ENOTSOCK),
            },
            FileBackend::Fuzz(fuzz_endpoint) => match &mut self.data {
                ReadData::BasicSlice(data) => {
                    let read = cmp::min(
                        data.len(),
                        state.global.fuzz_input.len() - fuzz_endpoint.borrow().read_idx,
                    );
                    data.copy_from_slice(
                        &state.global.fuzz_input[fuzz_endpoint.borrow().read_idx
                            ..fuzz_endpoint.borrow().read_idx + read]);
                    fuzz_endpoint.borrow_mut().read_idx += read;

                    Outcome::Success(read)
                }
                ReadData::Iovec(data) => {
                    let mut total_read = 0;
                    for s in data.iter_mut() {
                        let read = cmp::min(
                            s.len(),
                            state.global.fuzz_input.len() - fuzz_endpoint.borrow().read_idx,
                        );
                        s.copy_from_slice(
                            &state.global.fuzz_input[fuzz_endpoint.borrow().read_idx
                                ..fuzz_endpoint.borrow().read_idx + read],
                        );
                        fuzz_endpoint.borrow_mut().read_idx += read;
                        total_read += read;
                    }

                    Outcome::Success(total_read)
                }
                ReadData::File(data) => {
                    let mut offset = data
                        .offset
                        .unwrap_or(fuzz_endpoint.borrow().read_idx as i64)
                        as usize;
                    let mut total_read = 0;

                    if offset > state.global.fuzz_input.len() {
                        return Outcome::Success(0);
                    }

                    for s in data.buf.iter_mut() {
                        let read = cmp::min(s.len(), state.global.fuzz_input.len() - offset);
                        s.copy_from_slice(&state.global.fuzz_input[offset..offset + read]);
                        offset += read;
                        total_read += read;
                    }

                    if data.offset.is_none() {
                        fuzz_endpoint.borrow_mut().read_idx = offset;
                    }

                    Outcome::Success(total_read)
                }
                ReadData::Socket(_, _) => Outcome::Error(Errno::ENOTSOCK),
            },
        }
    }
}

pub struct FileWriteEvent<'a> {
    fd: Descriptor,
    data: WriteData<'a>,
    cow_created: bool,
}

impl<'a> FileWriteEvent<'a> {
    #[inline]
    pub fn new(fd: Descriptor, data: WriteData<'a>) -> Self {
        Self {
            fd,
            data,
            cow_created: false,
        }
    }
}

impl Event for FileWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(fd_info) = state.local.fds.get(&self.fd) else {
            return Outcome::Error(Errno::get_errno());
        };

        let FdResource::File(open_file) = &fd_info.resource else {
            unreachable!("non-file fd passed to FileReadEvent")
        };

        let file = open_file.borrow().file.clone();

        let file_ref = file.borrow();
        match &file_ref.backend {
            FileBackend::Passthrough => todo!(),
            FileBackend::Peered(_) => todo!(),
            FileBackend::Feedback(_) => {
                // TODO: edit modify time here
                // file_info.mtime

                let fd = if let Some(cow_id) = file_ref.cow {
                    let Some(cow_info) = state.local.pasture.get(&cow_id) else {
                        self.cow_created = true;
                        return Outcome::RunTask(
                            Task::CreateCow(CreateCowTask(CreateCowSource::Existing(cow_id))),
                            YieldUntil::Reschedule(Duration::ZERO),
                        )
                    };

                    cow_info.memfd
                } else {
                    self.cow_created = true;
                    return Outcome::RunTask(
                        Task::CreateCow(CreateCowTask(CreateCowSource::New(file_ref.path.clone(), file_ref.mode))),
                        YieldUntil::Reschedule(Duration::ZERO),
                    )
                };

                if self.cow_created {
                    // Update the file offset to be appropriate
                    let offset = unsafe { libc::lseek(self.fd.as_raw_fd(), 0, libc::SEEK_CUR) };
                    assert!(offset >= 0);

                    open_file.borrow_mut().offset = offset as usize;
                }

                let offset = open_file.borrow().offset;

                match &self.data {
                    WriteData::BasicSlice(slice) => {
                        let iov = IoSlice::new(slice);

                        let written = unsafe {
                            libc::pwritev(fd, iov.as_ptr().cast::<libc::iovec>(), 1, offset as i64)
                        };
                        if written < 0 {
                            let e = Errno::get_errno();
                            log::warn!(
                                "pwritev() failed with {} when reading data from file backend",
                                e
                            );
                            return Outcome::Error(e);
                        }

                        open_file.borrow_mut().offset += written as usize;

                        Outcome::Success(written as usize)
                    }
                    WriteData::Iovec(data) => {
                        let written = unsafe {
                            libc::pwritev(
                                fd,
                                data.as_ptr().cast::<libc::iovec>(),
                                data.len() as i32,
                                offset as i64,
                            )
                        };
                        if written < 0 {
                            let e = Errno::get_errno();
                            log::warn!(
                                "pwritev() failed with {} when reading data from file backend",
                                e
                            );
                            return Outcome::Error(e);
                        }

                        open_file.borrow_mut().offset += written as usize;

                        Outcome::Success(written as usize)
                    }
                    WriteData::File(file_read_data) => {
                        // TODO: handle flags
                        let data = &file_read_data.buf;
                        let offset = file_read_data
                            .offset
                            .unwrap_or(open_file.borrow().offset as i64);
                        let written = unsafe {
                            libc::pwritev(
                                fd,
                                data.as_ptr().cast::<libc::iovec>(),
                                data.len() as i32,
                                offset as i64,
                            )
                        };
                        if written < 0 {
                            let e = Errno::get_errno();
                            log::warn!(
                                "pwritev() failed with {} when reading data from file backend",
                                e
                            );
                            return Outcome::Error(e);
                        }

                        open_file.borrow_mut().offset += written as usize;

                        Outcome::Success(written as usize)
                    }
                    WriteData::Socket(_, _) => return Outcome::Error(Errno::ENOTSOCK),
                }
            }
            FileBackend::Plugin(_plugin_endpoint_id) => {
                unimplemented!("file plugins not implemented")
            }
            FileBackend::Sink => match &self.data {
                WriteData::BasicSlice(slice) => Outcome::Success(slice.len()),
                WriteData::Iovec(data) => Outcome::Success(data.iter().map(|s| s.len()).sum()),
                WriteData::File(data) => Outcome::Success(data.buf.iter().map(|s| s.len()).sum()),
                WriteData::Socket(_, _) => return Outcome::Error(Errno::ENOTSOCK),
            },
            FileBackend::NullSink => match &self.data {
                WriteData::BasicSlice(slice) => Outcome::Success(slice.len()),
                WriteData::Iovec(data) => Outcome::Success(data.iter().map(|s| s.len()).sum()),
                WriteData::File(data) => Outcome::Success(data.buf.iter().map(|s| s.len()).sum()),
                WriteData::Socket(_, _) => return Outcome::Error(Errno::ENOTSOCK),
            },
            FileBackend::Fuzz(_) => match &self.data {
                WriteData::BasicSlice(slice) => Outcome::Success(slice.len()),
                WriteData::Iovec(data) => Outcome::Success(data.iter().map(|s| s.len()).sum()),
                WriteData::File(data) => Outcome::Success(data.buf.iter().map(|s| s.len()).sum()),
                WriteData::Socket(_, _) => return Outcome::Error(Errno::ENOTSOCK),
            },
        }
    }
}

pub enum ChangeDirectorySource {
    Path(FilePath<MAX_PATH_LEN>),
    Directory(RawFd),
}

pub struct ChangeDirectoryEvent {
    source: ChangeDirectorySource,
}

impl ChangeDirectoryEvent {
    #[inline]
    pub fn new(source: ChangeDirectorySource) -> Self {
        Self { source }
    }
}

impl Event for ChangeDirectoryEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &self.source {
            ChangeDirectorySource::Path(file_path) => {
                let cstr = file_path.as_cstr();

                let dir = unsafe { libc::opendir(cstr.as_ptr()) };

                if !dir.is_null() {
                    assert_eq!(unsafe { libc::closedir(dir) }, 0);

                    if file_path.is_absolute() {
                        state.local.working_directory = file_path.clone();
                    } else {
                        let Ok(path) = state.local.working_directory.clone().concat(file_path)
                        else {
                            log::error!("relative directory too long when converted to absolute");
                            return Outcome::Error(Errno::EINVAL);
                        };

                        state.local.working_directory = path;
                    }

                    Outcome::Success(())
                } else {
                    Outcome::Error(Errno::get_errno())
                }
            }
            ChangeDirectorySource::Directory(dirfd) => {
                let Some(DescriptorInfo {
                    resource: FdResource::Directory(dir),
                    ..
                }) = state.local.fds.get(&Descriptor::from_raw_fd(*dirfd))
                else {
                    log::debug!("`fchdir` called with unrecognized fd");
                    #[cfg(not(feature = "passthroughfs"))]
                    return Outcome::Error(Errno::EBADF);
                    #[cfg(feature = "passthroughfs")]
                    return match unsafe { libc::fchdir(*dirfd) } {
                        0 => Outcome::Success(()),
                        _ => Outcome::Error(Errno::get_errno()),
                    };
                };

                state.local.working_directory = dir.borrow().path.clone();
                Outcome::Success(())
            }
        }
    }
}

pub enum ChangeOwnerSource {
    Path(FilePath<MAX_PATH_LEN>),
    Descriptor(RawFd),
    PathAt(FilePath<MAX_PATH_LEN>, RawFd),
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct ChangeOwnerFlags: libc::c_int {
        const AT_EMPTY_PATH = libc::AT_EMPTY_PATH;
        const AT_SYMLINK_NOFOLLOW = libc::AT_SYMLINK_NOFOLLOW;
    }
}

pub struct ChangeOwnerEvent {
    source: ChangeOwnerSource,
    owner: libc::uid_t,
    group: libc::gid_t,
    flags: ChangeOwnerFlags,
}

impl ChangeOwnerEvent {
    #[inline]
    pub fn new(
        source: ChangeOwnerSource,
        owner: libc::uid_t,
        group: libc::gid_t,
        flags: ChangeOwnerFlags,
    ) -> Self {
        Self {
            source,
            owner,
            group,
            flags,
        }
    }
}

impl Event for ChangeOwnerEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: handle flags

        let path = match &self.source {
            ChangeOwnerSource::Path(path) => {
                if path.is_absolute() {
                    path.clone()
                } else {
                    let Ok(filepath) = state.local.working_directory.clone().concat(&path) else {
                        log::error!("working directory and relative path of `chown` was too long");
                        return Outcome::Error(Errno::EINVAL);
                    };

                    filepath
                }
            }
            ChangeOwnerSource::Descriptor(fd) => {
                let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                    #[cfg(not(feature = "passthroughfs"))]
                    return Outcome::Error(Errno::EBADF);
                    #[cfg(feature = "passthroughfs")]
                    return match unsafe { libc::fchown(*fd, self.owner, self.group) } {
                        0 => Outcome::Success(()),
                        _ => Outcome::Error(Errno::get_errno()),
                    };
                };

                match &fd_info.resource {
                    FdResource::File(open_file) => {
                        let file_info = open_file.borrow().file.clone();
                        let path = file_info.borrow().path.clone();
                        path
                    }
                    FdResource::Directory(dir) => dir.borrow().path.clone(),
                    _ => return Outcome::Error(Errno::ENOTDIR),
                }
            }
            ChangeOwnerSource::PathAt(file_path, fd) => {
                let dir_path = if *fd == libc::AT_FDCWD {
                    state.local.working_directory.clone()
                } else {
                    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                        #[cfg(not(feature = "passthroughfs"))]
                        return Outcome::Error(Errno::EBADF);
                        #[cfg(feature = "passthroughfs")]
                        return match unsafe {
                            libc::fchownat(
                                *fd,
                                file_path.as_cstr().as_ptr(),
                                self.owner,
                                self.group,
                                self.flags.bits(),
                            )
                        } {
                            0 => Outcome::Success(()),
                            _ => Outcome::Error(Errno::get_errno()),
                        };
                    };

                    match &fd_info.resource {
                        FdResource::File(open_file) => {
                            let file_info = open_file.borrow().file.clone();
                            let path = file_info.borrow().path.clone();
                            path
                        }
                        FdResource::Directory(dir) => dir.borrow().path.clone(),
                        _ => return Outcome::Error(Errno::ENOTDIR),
                    }
                };

                let Ok(abs_path) = dir_path.clone().concat(&file_path) else {
                    panic!("working directory and relative path of ChangeOwnerEvent was too long")
                };

                abs_path
            }
        };

        if !state.global.file_paths.contains_key(&path) {
            return Outcome::Error(Errno::ENOENT);
        }

        // TODO: implement file ownership tracking here

        Outcome::Success(())
    }
}

pub enum ChangeModeSource {
    Path(FilePath<MAX_PATH_LEN>),
    Descriptor(RawFd),
    PathAt(FilePath<MAX_PATH_LEN>, RawFd),
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct ChangeModeFlags: libc::c_int {
        const AT_EMPTY_PATH = libc::AT_EMPTY_PATH;
        const AT_SYMLINK_NOFOLLOW = libc::AT_SYMLINK_NOFOLLOW;
    }
}

pub struct ChangeModeEvent {
    source: ChangeModeSource,
    mode: libc::mode_t,
    flags: ChangeModeFlags,
}

impl ChangeModeEvent {
    #[inline]
    pub fn new(source: ChangeModeSource, mode: libc::mode_t, flags: ChangeModeFlags) -> Self {
        Self {
            source,
            mode,
            flags,
        }
    }
}

impl Event for ChangeModeEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: handle flags

        let path = match &self.source {
            ChangeModeSource::Path(path) => {
                if path.is_absolute() {
                    path.clone()
                } else {
                    let Ok(filepath) = state.local.working_directory.clone().concat(&path) else {
                        log::error!("working directory and relative path of `chown` was too long");
                        return Outcome::Error(Errno::EINVAL);
                    };

                    filepath
                }
            }
            ChangeModeSource::Descriptor(fd) => {
                let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                    #[cfg(not(feature = "passthroughfs"))]
                    return Outcome::Error(Errno::EBADF);
                    #[cfg(feature = "passthroughfs")]
                    return match unsafe { libc::fchmod(*fd, self.mode) } {
                        0 => Outcome::Success(()),
                        _ => Outcome::Error(Errno::get_errno()),
                    };
                };

                match &fd_info.resource {
                    FdResource::File(open_file) => open_file.borrow().file.borrow().path.clone(),
                    FdResource::Directory(dir) => dir.borrow().path.clone(),
                    _ => return Outcome::Error(Errno::ENOTDIR),
                }
            }
            ChangeModeSource::PathAt(file_path, fd) => {
                let dir_path = if *fd == libc::AT_FDCWD {
                    state.local.working_directory.clone()
                } else {
                    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                        #[cfg(not(feature = "passthroughfs"))]
                        return Outcome::Error(Errno::EBADF);
                        #[cfg(feature = "passthroughfs")]
                        return match unsafe {
                            libc::fchmodat(
                                *fd,
                                file_path.as_cstr().as_ptr(),
                                self.mode,
                                self.flags.bits(),
                            )
                        } {
                            0 => Outcome::Success(()),
                            _ => Outcome::Error(Errno::get_errno()),
                        };
                    };

                    match &fd_info.resource {
                        FdResource::File(open_file) => {
                            open_file.borrow().file.borrow().path.clone()
                        }
                        FdResource::Directory(dir) => dir.borrow().path.clone(),
                        _ => return Outcome::Error(Errno::ENOTDIR),
                    }
                };

                let Ok(abs_path) = dir_path.clone().concat(&file_path) else {
                    panic!("working directory and relative path of ChangeOwnerEvent was too long")
                };

                abs_path
            }
        };

        if !state.global.file_paths.contains_key(&path) {
            return Outcome::Error(Errno::ENOENT);
        }

        // TODO: implement file mode tracking here

        Outcome::Success(())
    }
}

pub enum AccessSource {
    Path(FilePath<MAX_PATH_LEN>),
    PathAt(FilePath<MAX_PATH_LEN>, RawFd),
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct AccessFlags: libc::c_int {
        const AT_EACCESS = libc::AT_EACCESS;
        const AT_SYMLINK_NOFOLLOW = libc::AT_SYMLINK_NOFOLLOW;
    }
}

pub struct AccessEvent {
    source: AccessSource,
    mode: libc::c_int,
    flags: AccessFlags,
}

impl AccessEvent {
    #[inline]
    pub fn new(source: AccessSource, mode: libc::c_int, flags: AccessFlags) -> Self {
        Self {
            source,
            mode,
            flags,
        }
    }
}

impl Event for AccessEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: handle flags

        let path = match &self.source {
            AccessSource::Path(path) => {
                if path.is_absolute() {
                    path.clone()
                } else {
                    let Ok(filepath) = state.local.working_directory.clone().concat(&path) else {
                        log::error!("working directory and relative path of `chown` was too long");
                        return Outcome::Error(Errno::EINVAL);
                    };

                    filepath
                }
            }
            AccessSource::PathAt(file_path, fd) => {
                let dir_path = if *fd == libc::AT_FDCWD {
                    state.local.working_directory.clone()
                } else {
                    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                        #[cfg(not(feature = "passthroughfs"))]
                        return Outcome::Error(Errno::EBADF);
                        #[cfg(feature = "passthroughfs")]
                        return match unsafe {
                            libc::faccessat(
                                *fd,
                                file_path.as_cstr().as_ptr(),
                                self.mode,
                                self.flags.bits(),
                            )
                        } {
                            0 => Outcome::Success(()),
                            _ => Outcome::Error(Errno::get_errno()),
                        };
                    };

                    match &fd_info.resource {
                        FdResource::File(open_file) => {
                            open_file.borrow().file.borrow().path.clone()
                        }
                        FdResource::Directory(dir) => dir.borrow().path.clone(),
                        _ => return Outcome::Error(Errno::ENOTDIR),
                    }
                };

                let Ok(abs_path) = dir_path.clone().concat(&file_path) else {
                    panic!("working directory and relative path of AccessEvent was too long")
                };

                abs_path
            }
        };

        if !state.global.file_paths.contains_key(&path) {
            return Outcome::Error(Errno::ENOENT);
        }

        // TODO: implement file mode access check here

        Outcome::Success(())
    }
}

pub enum StatSource {
    Path(FilePath<MAX_PATH_LEN>),
    Descriptor(RawFd),
    PathAt(FilePath<MAX_PATH_LEN>, RawFd),
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct StatFlags: libc::c_int {
        const AT_EMPTY_PATH = libc::AT_EMPTY_PATH;
        const AT_NO_AUTOMOUNT = libc::AT_NO_AUTOMOUNT;
        const AT_SYMLINK_NOFOLLOW = libc::AT_SYMLINK_NOFOLLOW;
    }
}

pub struct StatEvent<'a> {
    source: StatSource,
    flags: StatFlags,
    stat_buf: &'a mut libc::stat,
}

impl<'a> StatEvent<'a> {
    #[inline]
    pub fn new(source: StatSource, stat_buf: &'a mut libc::stat, flags: StatFlags) -> Self {
        Self {
            source,
            flags,
            stat_buf,
        }
    }
}

impl Event for StatEvent<'_> {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: handle flags

        let path = match &self.source {
            StatSource::Path(path) => {
                if path.is_absolute() {
                    path.clone()
                } else {
                    let Ok(filepath) = state.local.working_directory.clone().concat(&path) else {
                        log::error!("working directory and relative path of `stat` was too long");
                        return Outcome::Error(Errno::EINVAL);
                    };

                    filepath
                }
            }
            StatSource::Descriptor(fd) => {
                let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                    return Outcome::Error(Errno::EBADF);
                };

                match &fd_info.resource {
                    FdResource::File(open_file) => open_file.borrow().file.borrow().path.clone(),
                    FdResource::Directory(dir) => dir.borrow().path.clone(),
                    _ => return Outcome::Error(Errno::EBADF),
                }
            }
            StatSource::PathAt(file_path, fd) => {
                let dir_path = if *fd == libc::AT_FDCWD {
                    state.local.working_directory.clone()
                } else {
                    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*fd)) else {
                        return Outcome::Error(Errno::EBADF);
                    };

                    match &fd_info.resource {
                        FdResource::File(open_file) => {
                            open_file.borrow().file.borrow().path.clone()
                        }
                        FdResource::Directory(dir) => dir.borrow().path.clone(),
                        _ => return Outcome::Error(Errno::EBADF),
                    }
                };

                let Ok(abs_path) = dir_path.clone().concat(&file_path) else {
                    panic!("working directory and relative path of StatEvent was too long")
                };

                abs_path
            }
        };

        let file_info = match state.global.file_paths.get(&path) {
            Some(file_id) => file_id,
            None => {
                if unsafe { libc::access(path.as_cstr().as_ptr(), libc::F_OK) } != 0 {
                    return Outcome::Error(Errno::ENOENT);
                }

                let inode = state.global.next_inode();
                let mode = state.local.umask;
                let uid = state.global.uid;
                let gid = state.global.gid;
                let current_time = state.global.current_time;

                let file_info = Rc::new_in(
                    RefCell::new(FileInfo {
                        path: path.clone(),
                        cow: None,
                        dev_id: 0xfe01,
                        inode,
                        mode,
                        nlink: 1,
                        uid,
                        gid,
                        atime: current_time,
                        btime: current_time,
                        mtime: current_time,
                        ctime: current_time, // TODO: edit these
                        backend: FileBackend::Feedback(FileFeedback {}),
                    }),
                    fizzle_alloc(),
                );
                let Ok(_) = state.global.file_paths.insert(path.clone(), file_info) else {
                    panic!()
                };
                state.global.file_paths.get(&path).unwrap()
            }
        };

        let file_info_ref = file_info.borrow();
        let cow_opt = file_info_ref.cow;
        drop(file_info_ref);

        let size = match cow_opt {
            Some(cow_id) => {
                let Some(cow_info) = state.local.pasture.get(&cow_id) else {
                    // Fetch CoW ID for this process and retry
                    return Outcome::RunTask(
                        Task::CreateCow(CreateCowTask(CreateCowSource::Existing(cow_id))),
                        YieldUntil::Reschedule(Duration::ZERO),
                    )
                };

                unsafe {
                    let mut stat: MaybeUninit<libc::stat> = MaybeUninit::uninit();
                    assert_eq!(
                        libc::fstat(cow_info.memfd, ptr::addr_of_mut!(stat).cast::<libc::stat>()),
                        0
                    );
                    let stat = stat.assume_init();
                    stat.st_size as usize
                }
            }
            None => unsafe {
                let mut stat: MaybeUninit<libc::stat> = MaybeUninit::uninit();
                assert_eq!(
                    libc::stat(
                        path.as_cstr().as_ptr(),
                        ptr::addr_of_mut!(stat).cast::<libc::stat>()
                    ),
                    0
                );
                let stat = stat.assume_init();
                stat.st_size as usize
            },
        };

        let file_ref = file_info.borrow();
        self.stat_buf.st_atime = file_ref.atime.as_secs() as i64;
        self.stat_buf.st_atime_nsec = file_ref.atime.subsec_nanos() as i64;
        self.stat_buf.st_blksize = 4096;
        self.stat_buf.st_blocks = (size / 4096) as i64 + 1;
        self.stat_buf.st_ctime = file_ref.ctime.as_secs() as i64;
        self.stat_buf.st_ctime_nsec = file_ref.ctime.subsec_nanos() as i64;
        self.stat_buf.st_dev = file_ref.dev_id;
        self.stat_buf.st_gid = file_ref.gid;
        self.stat_buf.st_ino = file_ref.inode;
        self.stat_buf.st_mode = file_ref.mode.bits();
        self.stat_buf.st_mtime = file_ref.mtime.as_secs() as i64;
        self.stat_buf.st_mtime_nsec = file_ref.mtime.subsec_nanos() as i64;
        self.stat_buf.st_nlink = file_ref.nlink as u64;
        self.stat_buf.st_rdev = 0;
        self.stat_buf.st_size = size as i64;
        self.stat_buf.st_uid = file_ref.uid;

        Outcome::Success(())
    }
}

pub struct UmaskEvent {
    umask: AccessMode,
}

impl UmaskEvent {
    #[inline]
    pub fn new(umask: AccessMode) -> Self {
        Self { umask }
    }
}

impl Event for UmaskEvent {
    type Success = AccessMode;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let prev = state.local.umask;
        state.local.umask = self.umask;
        Outcome::Success(prev)
    }
}

pub enum RenameSrcDst {
    Path(FilePath<MAX_PATH_LEN>, FilePath<MAX_PATH_LEN>),
    PathAt(FilePath<MAX_PATH_LEN>, RawFd, FilePath<MAX_PATH_LEN>, RawFd),
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct RenameFlags: libc::c_uint {
        const RENAME_EXCHANGE = libc::RENAME_EXCHANGE;
        const RENAME_NOREPLACE = libc::RENAME_NOREPLACE;
        const RENAME_WHITEOUT = libc::RENAME_WHITEOUT;
    }
}

pub struct RenameEvent {
    src_dst: RenameSrcDst,
    flags: RenameFlags,
}

impl RenameEvent {
    #[inline]
    pub fn new(src_dst: RenameSrcDst, flags: RenameFlags) -> Self {
        Self { src_dst, flags }
    }
}

impl Event for RenameEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let (oldpath, newpath) = match &self.src_dst {
            RenameSrcDst::Path(rel_oldpath, rel_newpath) => {
                let oldpath = if rel_oldpath.is_absolute() {
                    rel_oldpath.clone()
                } else {
                    let Ok(filepath) = state.local.working_directory.clone().concat(&rel_oldpath)
                    else {
                        log::error!("working directory and relative path of oldpath for `rename` was too long");
                        return Outcome::Error(Errno::ENAMETOOLONG);
                    };

                    filepath
                };

                let newpath = if rel_newpath.is_absolute() {
                    rel_newpath.clone()
                } else {
                    let Ok(filepath) = state.local.working_directory.clone().concat(&rel_newpath)
                    else {
                        log::error!("working directory and relative path of newpath for `rename` was too long");
                        return Outcome::Error(Errno::ENAMETOOLONG);
                    };

                    filepath
                };

                (oldpath, newpath)
            }
            RenameSrcDst::PathAt(rel_oldpath, oldfd, rel_newpath, newfd) => {
                let olddir_path = if *oldfd == libc::AT_FDCWD {
                    state.local.working_directory.clone()
                } else {
                    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*oldfd))
                    else {
                        return Outcome::Error(Errno::EBADF);
                    };

                    match &fd_info.resource {
                        FdResource::File(open_file) => {
                            open_file.borrow().file.borrow().path.clone()
                        }
                        FdResource::Directory(dir) => dir.borrow().path.clone(),
                        _ => return Outcome::Error(Errno::EBADF),
                    }
                };

                let Ok(abs_oldpath) = olddir_path.concat(&rel_oldpath) else {
                    panic!("working directory and relative path of RenameEvent old filepath was too long")
                };

                let newdir_path = if *newfd == libc::AT_FDCWD {
                    state.local.working_directory.clone()
                } else {
                    let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(*newfd))
                    else {
                        return Outcome::Error(Errno::EBADF);
                    };

                    match &fd_info.resource {
                        FdResource::File(open_file) => {
                            open_file.borrow().file.borrow().path.clone()
                        }
                        FdResource::Directory(dir) => dir.borrow().path.clone(),
                        _ => return Outcome::Error(Errno::EBADF),
                    }
                };

                let Ok(abs_newpath) = newdir_path.concat(&rel_newpath) else {
                    panic!("working directory and relative path of RenameEvent new filepath was too long")
                };

                (abs_oldpath, abs_newpath)
            }
        };

        if let Some(new_file_info) = state.global.file_paths.get(&newpath) {
            if let Some(cow_id) = new_file_info.borrow().cow {
                // TODO: what to do here?
            };
        }

        if let Some(old_file_info) = state.global.file_paths.get(&oldpath) {
            // TODO: what to do here?
        }

        if self.flags.contains(RenameFlags::RENAME_NOREPLACE)
            && state.global.file_paths.contains_key(&newpath)
        {
            return Outcome::Error(Errno::EEXIST);
        }

        let replace_file_id = state.global.file_paths.remove(&newpath);

        let Some(move_file_info) = state.global.file_paths.remove(&oldpath) else {
            return Outcome::Error(Errno::ENOENT); // TODO: fix error code
        };

        if state
            .global
            .file_paths
            .insert(newpath.clone(), move_file_info.clone())
            .is_err()
        {
            panic!("failed to insert to file_paths")
        }
        move_file_info.borrow_mut().path = newpath;

        if self.flags.contains(RenameFlags::RENAME_EXCHANGE) {
            if let Some(file_info) = replace_file_id {
                if state
                    .global
                    .file_paths
                    .insert(oldpath.clone(), file_info.clone())
                    .is_err()
                {
                    panic!("failed to insert to file_paths")
                }
                file_info.borrow_mut().path = oldpath;
            }
        }

        Outcome::Success(())
    }
}
