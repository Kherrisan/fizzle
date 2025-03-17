use core::slice;
use std::ffi::CStr;
use std::ffi::VaList;
use std::io::IoSlice;
use std::io::IoSliceMut;
use std::ptr;

use fizzle_common::path::FilePath;

use crate::errno::Errno;
use crate::handlers::file::*;
use crate::handlers::filestream::*;
use crate::hook_macros;
use crate::scheduler::fizzle_singleton;
use crate::scheduler::Scheduler;

/*

hook_macros::hook! {
    unsafe fn inotify_init() => fizzle_inotify_init(_ctx) {
        log::error!("`inotify_init()` not implemented by Fizzle");
        unimplemented!("inotify_init()")
    }
}

hook_macros::hook! {
    unsafe fn inotify_init1(
        _flags: libc::c_int
    ) => fizzle_inotify_init1(_ctx) {
        log::error!("`inotify_init1()` not implemented by Fizzle");
        unimplemented!("inotify_init1()")
    }
}

hook_macros::hook! {
    unsafe fn fanotify_init(
        _flags: libc::c_uint,
        _event_f_flags: libc::c_uint
    ) => fizzle_fanotify_init(_ctx) {
        log::error!("`fanotify_init()` not implemented by Fizzle");
        unimplemented!("fanotify_init()")
    }
}

hook_macros::hook! {
    unsafe fn fdopen(
        fd: libc::c_int,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fdopen(ctx) {
        // SAFETY: caller guarantees `mode` points to a null-terminated string
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("fdopen(fd={}, mode={:?}) -> ...", fd, mode_cstr);

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, None)) {
            Ok(mut file_ptr) => {
                crate::strace!("fdopen(fd={}, mode={:?}) -> {:?}", fd, mode_cstr, file_ptr);
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("fdopen(fd={}, mode={:?}) -> NULL ({})", fd, mode_cstr, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fopen(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("fopen(pathname={:?}, mode={:?}) -> ...", path_cstr, mode_cstr);

        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        let fd = match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(path),stream_mode.flags, Some(access_mode))) {
            Ok(fd) => fd,
            Err(e) => {
                crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
                e.set_errno();
                return ptr::null_mut()
            }
        };

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, None)) {
            Ok(mut file_ptr) => {
                crate::strace!("fopen(pathname={:?}, mode={:?}) -> {:?}", path_cstr, mode_cstr, file_ptr);
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fopen64(
        pathname: *const libc::c_char,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fopen64(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("fopen64(pathname={:?}, mode={:?}) -> ...", path_cstr, mode_cstr);

        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        let fd = match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(path),stream_mode.flags, Some(access_mode))) {
            Ok(fd) => fd,
            Err(e) => {
                crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
                e.set_errno();
                return ptr::null_mut()
            }
        };

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, None)) {
            Ok(mut file_ptr) => {
                crate::strace!("fopen64(pathname={:?}, mode={:?}) -> {:?}", path_cstr, mode_cstr, file_ptr);
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn _IO_fopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle__IO_fopen(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("_IO_fopen(pathname={:?}, mode={:?}) -> ...", path_cstr, mode_cstr);

        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("_IO_fopen(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("_IO_fopen(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        let fd = match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(path),stream_mode.flags, Some(access_mode))) {
            Ok(fd) => fd,
            Err(e) => {
                crate::strace!("_IO_fopen(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
                e.set_errno();
                return ptr::null_mut()
            }
        };

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, None)) {
            Ok(mut file_ptr) => {
                crate::strace!("_IO_fopen(pathname={:?}, mode={:?}) -> {:?}", path_cstr, mode_cstr, file_ptr);
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("_IO_fopen(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn freopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> *mut libc::FILE => fizzle_freopen(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> ...", path_cstr, mode_cstr, stream);

        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr, stream);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr, stream);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr, stream);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamFlushEvent::new(Some(stream_ptr))) {
            crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL ({})", path_cstr, mode_cstr, stream, e);
            e.set_errno();
            return ptr::null_mut()
        }

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamCloseEvent::new(&stream_ptr)) {
            crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL ({})", path_cstr, mode_cstr, stream, e);
            e.set_errno();
            return ptr::null_mut()
        }

        let fd = match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(path),stream_mode.flags, Some(access_mode))) {
            Ok(fd) => fd,
            Err(e) => {
                crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL ({})", path_cstr, mode_cstr, stream, e);
                e.set_errno();
                return ptr::null_mut()
            }
        };

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, Some(stream_ptr))) {
            Ok(mut file_ptr) => {
                crate::strace!("freopen(fd={}, mode={:?}, stream={:?}) -> {:?}", fd, mode_cstr, stream_ptr, file_ptr);
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("freopen(fd={}, mode={:?}, stream={:?}) -> NULL ({})", fd, mode_cstr, stream_ptr, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn freopen64(
        pathname: *const libc::c_char,
        mode: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> *mut libc::FILE => fizzle_freopen64(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> ...", path_cstr, mode_cstr, stream);

        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr, stream);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr, stream);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr, stream);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamFlushEvent::new(Some(stream_ptr))) {
            crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL ({})", path_cstr, mode_cstr, stream, e);
            e.set_errno();
            return ptr::null_mut()
        }

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamCloseEvent::new(&stream_ptr)) {
            crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL ({})", path_cstr, mode_cstr, stream, e);
            e.set_errno();
            return ptr::null_mut()
        }

        let fd = match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(path),stream_mode.flags, Some(access_mode))) {
            Ok(fd) => fd,
            Err(e) => {
                crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL ({})", path_cstr, mode_cstr, stream, e);
                e.set_errno();
                return ptr::null_mut()
            }
        };

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, Some(stream_ptr))) {
            Ok(mut file_ptr) => {
                crate::strace!("freopen64(fd={}, mode={:?}, stream={:?}) -> {:?}", fd, mode_cstr, stream_ptr, file_ptr);
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("freopen64(fd={}, mode={:?}, stream={:?}) -> NULL ({})", fd, mode_cstr, stream_ptr, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn _IO_file_fopen(
        stream: *mut libc::FILE,
        pathname: *const libc::c_char,
        mode: *const libc::c_char,
        is32: libc::c_int
    ) -> *mut libc::FILE => fizzle__IO_file_fopen(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> ...", stream, path_cstr, mode_cstr, is32);

        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL (EINVAL)", stream, path_cstr, mode_cstr, is32);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL (EINVAL)", stream, path_cstr, mode_cstr, is32);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL (EINVAL)", stream, path_cstr, mode_cstr, is32);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamFlushEvent::new(Some(stream_ptr))) {
            crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL ({})", stream, path_cstr, mode_cstr, is32, e);
            e.set_errno();
            return ptr::null_mut()
        }

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamCloseEvent::new(&stream_ptr)) {
            crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL ({})", stream, path_cstr, mode_cstr, is32, e);
            e.set_errno();
            return ptr::null_mut()
        }

        let fd = match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(path),stream_mode.flags, Some(access_mode))) {
            Ok(fd) => fd,
            Err(e) => {
                crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL ({})", stream, path_cstr, mode_cstr, is32, e);
                e.set_errno();
                return ptr::null_mut()
            }
        };

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, FileStreamCreateEvent::new(source, stream_mode, Some(stream_ptr))) {
            Ok(mut file_ptr) => {
                crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> {:?}", stream, path_cstr, mode_cstr, is32, file_ptr.as_raw());
                file_ptr.as_raw()
            },
            Err(e) => {
                crate::strace!("_IO_file_fopen(stream={:?}, pathname={:?}, mode={:?}, is32={}) -> NULL ({})", stream, path_cstr, mode_cstr, is32, e);
                e.set_errno();
                ptr::null_mut()
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fmemopen(
        _buf: *mut libc::c_void,
        _size: libc::size_t,
        _mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fmemopen(_ctx) {
        unimplemented!("fmemopen()")
        // hook_macros::real!(fmemopen)(buf, size, mode)
    }
}

hook_macros::hook! {
    unsafe fn open_memstream(
        _ptr: *mut *mut libc::c_char,
        _sizeloc: *mut libc::size_t
    ) -> *mut libc::FILE => fizzle_open_memstream(_ctx) {
        unimplemented!("open_memstream()")
    }
}

hook_macros::hook! {
    unsafe fn fclose(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fclose(ctx) {
        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fclose(stream={:?}) -> -1 (EINVAL)", stream);
            Errno::EINVAL.set_errno();
            return -1
        };

        if let Err(e) = Scheduler::handle_event(&mut ctx, FileStreamFlushEvent::new(Some(stream_ptr))) {
            crate::strace!("fclose(stream={:?}) -> -1 ({})", stream, e);
            e.set_errno();
            return -1
        }

        match Scheduler::handle_event(&mut ctx, FileStreamCloseEvent::new(&stream_ptr)) {
            Ok(()) => {
                crate::strace!("fclose(stream={:?}) -> 0", stream);
                0
            }
            Err(e) => {
                crate::strace!("fclose(stream={:?}) -> -1 ({})", stream, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fflush(stream: *mut libc::FILE) -> libc::c_int => fizzle_fflush(ctx) {
        let stream_ptr_opt = FilePtr::from_raw(stream);

        crate::strace!("fflush(stream={:?}) -> ...", stream);

        match Scheduler::handle_event(&mut ctx, FileStreamFlushEvent::new(stream_ptr_opt)) {
            Ok(()) => {
                crate::strace!("fflush(stream={:?}) -> 0", stream);
                0
            }
            Err(e) => {
                crate::strace!("fflush(stream={:?}) -> -1 ({})", stream, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fileno(stream: *mut libc::FILE) -> libc::c_int => fizzle_fileno(ctx) {
        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fileno(stream={:?}) -> -1 (EINVAL)", stream);
            Errno::EINVAL.set_errno();
            return -1
        };

        crate::strace!("fileno(stream={:?}) -> ...", stream);

        match Scheduler::handle_event(&mut ctx, FileStreamDescriptorEvent::new(stream_ptr)) {
            Ok(fd) => {
                crate::strace!("fileno(stream={:?}) -> {}", stream, fd);
                fd
            }
            Err(e) => {
                crate::strace!("fileno(stream={:?}) -> -1 ({})", stream, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fwrite(
        ptr: *const libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwrite(ctx) {
        crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> ...", ptr, size, nmemb, stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 (EINVAL)", ptr, size, nmemb, stream);
            Errno::EINVAL.set_errno();
            return 0
        };

        let buf = slice::from_raw_parts(ptr.cast::<u8>(), size * nmemb);
        let io_slice = IoSlice::new(buf);

        match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, size)) {
            Ok(written) => {
                crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, written);
                written
            }
            Err(e) => {
                crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 ({})", ptr, size, nmemb, stream, e);
                e.set_errno(); // TODO: this doesn't need to be set
                0
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fread(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fread(ctx) {
        crate::strace!("fread(ptr={:?}, size={}, nmemb={}, stream={:?}) -> ...", ptr, size, nmemb, stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fread(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 (EINVAL)", ptr, size, nmemb, stream);
            Errno::EINVAL.set_errno();
            return 0
        };

        let buf = slice::from_raw_parts_mut(ptr.cast::<u8>(), size * nmemb);
        let mut io_slice = IoSliceMut::new(buf);

        match Scheduler::handle_event(&mut ctx, FileStreamReadEvent::new(stream_ptr, &mut io_slice, size)) {
            Ok(written) => {
                crate::strace!("fread(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, written);
                written
            }
            Err(e) => {
                crate::strace!("fread(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 ({})", ptr, size, nmemb, stream, e);
                e.set_errno(); // TODO: this doesn't need to be set
                0
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetc(
        _stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fgetc(_ctx) {
        panic!("fgetc() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn fgets(
        s: *mut libc::c_char,
        size: libc::c_int,
        _stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fgets(_ctx) {
        panic!("fgets() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn ungetc(
        _c: libc::c_int,
        _stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ungetc(_ctx) {
        panic!("ungetc() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn fputc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputc(ctx) {
        crate::strace!("fputc(c={}, stream={:?}) -> ...", c, stream);
        let c = c as u8;

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fputc(c={}, stream={:?}) -> ...", c, stream);
            Errno::EINVAL.set_errno();
            return libc::EOF
        };

        let io_slice = IoSlice::new(slice::from_ref(&c));

        match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, 1)) {
            Ok(written) => {
                crate::strace!("fputc(c={:?}, stream={:?}) -> {}", c, stream, written);
                written.try_into().unwrap()
            }
            Err(e) => {
                crate::strace!("fputc(c={:?}, stream={:?}) -> EOF ({})", c, stream, e);
                e.set_errno(); // TODO: this doesn't need to be set
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn putchar(
        c: libc::c_int
    ) -> libc::c_int => fizzle_putchar(ctx) {
        crate::strace!("putchar(c={}) -> ...", c);

        let c = c as u8;

        let stream_ptr = FilePtr::from_raw(unsafe { crate::stdout }).unwrap();

        let io_slice = IoSlice::new(slice::from_ref(&c));

        match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, 1)) {
            Ok(written) => {
                crate::strace!("putchar(c={:?}) -> {}", c, written);
                written.try_into().unwrap()
            }
            Err(e) => {
                crate::strace!("putchar(c={:?}) -> EOF ({})", c, e);
                e.set_errno(); // TODO: this doesn't need to be set
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fputs(
        s: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputs(ctx) {
        crate::strace!("fputs(s={:?}, stream={:?}) -> ...", s, stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fputs(s={:?}, stream={:?}) -> EOF (EINVAL)", s, stream);
            Errno::EINVAL.set_errno();
            return libc::EOF
        };

        let cstr = unsafe { CStr::from_ptr(s) };
        let buf = cstr.to_bytes();

        let io_slice = IoSlice::new(buf);

        match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, 1)) {
            Ok(written) => {
                crate::strace!("fputs(s={:?}, stream={:?}) -> {}", s, stream, written);
                written.try_into().unwrap()
            }
            Err(e) => {
                crate::strace!("fputs(s={:?}, stream={:?}) -> EOF ({})", s, stream, e);
                e.set_errno(); // TODO: this doesn't need to be set
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn puts(
        s: *const libc::c_char
    ) -> libc::c_int => fizzle_puts(ctx) {
        crate::strace!("puts(s={:?}) -> ...", s);

        let stream_ptr = FilePtr::from_raw(unsafe { crate::stdout }).unwrap();

        let cstr = unsafe { CStr::from_ptr(s) };
        let mut buf = Vec::from(cstr.to_bytes());
        buf.push(b'\n');

        let io_slice = IoSlice::new(buf.as_slice());

        match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, 1)) {
            Ok(written) => {
                crate::strace!("puts(s={:?}) -> {}", s, written);
                written.try_into().unwrap()
            }
            Err(e) => {
                crate::strace!("puts(s={:?}) -> EOF ({})", s, e);
                e.set_errno(); // TODO: this doesn't need to be set
                libc::EOF
            }
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct cookie_io_functions {
    #[allow(unused)]
    pub read: extern "C" fn(*mut libc::c_void, *mut libc::c_char, libc::size_t) -> libc::ssize_t,
    #[allow(unused)]
    pub write: extern "C" fn(*mut libc::c_void, *const libc::c_char, libc::size_t) -> libc::ssize_t,
    #[allow(unused)]
    pub seek: extern "C" fn(*mut libc::c_void, *mut libc::off64_t, libc::c_int) -> libc::c_int,
    #[allow(unused)]
    pub close: extern "C" fn(*mut libc::c_void) -> libc::c_int,
}

hook_macros::hook! {
    unsafe fn fopencookie(
        _cookie: *mut libc::c_void,
        _mode: *const libc::c_char,
        _io_funcs: cookie_io_functions
    ) -> *mut libc::FILE => fizzle_fopencookie(_ctx) {
        unimplemented!("fopencookie()")
        // hook_macros::real!(fopencookie)(cookie, mode, io_funcs)
    }
}

hook_macros::hook! {
    unsafe fn fseek(
        stream: *mut libc::FILE,
        offset: libc::c_long,
        whence: libc::c_int
    ) -> libc::c_int => fizzle_fseek(ctx) {
        unimplemented!("fseek()")
    }
}

hook_macros::hook! {
    unsafe fn ftell(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ftell(ctx) {
        unimplemented!("ftell()")
    }
}

hook_macros::hook! {
    unsafe fn rewind(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_rewind(ctx) {
        unimplemented!("frewind()")
    }
}

hook_macros::hook! {
    unsafe fn fgetpos(
        stream: *mut libc::FILE,
        pos: *mut libc::fpos_t
    ) -> libc::c_int => fizzle_fgetpos(ctx) {
        unimplemented!("fgetpos()")
    }
}

hook_macros::hook! {
    unsafe fn fsetpos(
        stream: *mut libc::FILE,
        pos: *const libc::fpos_t
    ) -> libc::c_int => fizzle_fsetpos(ctx) {
        unimplemented!("fsetpos()")
    }
}

hook_macros::hook! {
    unsafe fn clearerr(
        _stream: *mut libc::FILE
    ) => fizzle_clearerr(_ctx) {
        unimplemented!("clearerr()")
    }
}

hook_macros::hook! {
    unsafe fn feof(
        _stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_feof(_ctx) {
        unimplemented!("feof()")
    }
}

hook_macros::hook! {
    unsafe fn ferror(
        _stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ferror(_ctx) {
        unimplemented!("ferror()")
    }
}




hook_macros::hook! {
    unsafe fn __fbufsize(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fbufsize(ctx) {
        unimplemented!("__fbufsize()")
    }
}

hook_macros::hook! {
    unsafe fn __fpending(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fpending(ctx) {
        unimplemented!("__fpending()")
    }
}

hook_macros::hook! {
    unsafe fn __flbf(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_flbf(ctx) {
        unimplemented!("__flbf()")
    }
}

hook_macros::hook! {
    unsafe fn __freadable(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_freadable(ctx) {
        unimplemented!("__freadable()");
    }
}

hook_macros::hook! {
    unsafe fn __fwritable(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fwritable(ctx) {
        unimplemented!("__fwritable()")
    }
}

hook_macros::hook! {
    unsafe fn __freading(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_freading(ctx) {
        unimplemented!("__freading()")
    }
}

hook_macros::hook! {
    unsafe fn __fwriting(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwriting(ctx) {
        unimplemented!("__fwriting()")
    }
}

hook_macros::hook! {
    unsafe fn __fsetlocking(
        stream: *mut libc::FILE,
        lock_type: libc::c_int
    ) -> libc::c_int => fizzle_fsetlocking(ctx) {
        unimplemented!("__fsetlocking()")
    }
}

hook_macros::hook! {
    unsafe fn _flushlbf() => fizzle_flushlbf(_ctx) {
        unimplemented!("_flushlbf()")
    }
}

hook_macros::hook! {
    unsafe fn __fpurge(
        stream: *mut libc::FILE
    ) => fizzle_fpurge(ctx) {
        unimplemented!("__fpurge()")
    }
}

*/
