use core::slice;
use std::ffi::CStr;
use std::ptr;

use crate::errno::Errno;
use crate::handlers::file::{AccessMode, FileOpenFlags, SeekPosition};
use crate::handlers::filestream::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

const FSETLOCKING_QUERY: libc::c_int = 0;
const FSETLOCKING_INTERNAL: libc::c_int = 1;
const FSETLOCKING_BYCALLER: libc::c_int = 2;

hook_macros::hook! {
    unsafe fn fdopen(
        fd: libc::c_int,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fdopen(ctx) {
        // SAFETY: caller guarantees `mode` points to a null-terminated string
        let mode_cstr = CStr::from_ptr(mode);
        crate::strace!("fdopen(fd={}, mode={:?}) -> ...", fd, mode_cstr);

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            log::warn!("invalid or unrecognized `mode` encountered in fdopen() call");
            crate::strace!("fdopen(fd={}, mode={:?}) -> NULL (EINVAL)", fd, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, StreamCreateEvent::new(source, stream_mode, None)) {
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

        /*
        let Ok(path) = FilePath::from_cstr(path_cstr) else {
            crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };
        */

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        log::warn!("`fopen()` partially unimplemented");

        let fd = if stream_mode.flags.contains(FileOpenFlags::CREATE) {
            unsafe { libc::open(pathname, stream_mode.flags.bits(), access_mode.bits()) }
        } else {
            unsafe { libc::open(pathname, stream_mode.flags.bits()) }
        };

        if fd < 0 {
            let e = Errno::get_errno();
            crate::strace!("fopen(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
            e.set_errno();
            return ptr::null_mut()
        }

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, StreamCreateEvent::new(source, stream_mode, None)) {
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

        let mode_cstr = unsafe { CStr::from_ptr(mode) };

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let access_mode = AccessMode::USER_READ | AccessMode::USER_WRITE | AccessMode::GROUP_READ
                | AccessMode::GROUP_WRITE | AccessMode::OTHER_READ | AccessMode::OTHER_WRITE;

        log::warn!("`fopen64()` partially unimplemented");

        let fd = if stream_mode.flags.contains(FileOpenFlags::CREATE) {
            unsafe { libc::open(pathname, stream_mode.flags.bits(), access_mode.bits()) }
        } else {
            unsafe { libc::open(pathname, stream_mode.flags.bits()) }
        };

        if fd < 0 {
            let e = Errno::get_errno();
            crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL ({})", path_cstr, mode_cstr, e);
            e.set_errno();
            return ptr::null_mut()
        }

        let source = FileStreamSource::Descriptor(fd);
        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        match Scheduler::handle_event(&mut ctx, StreamCreateEvent::new(source, stream_mode, None)) {
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
    unsafe fn freopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> *mut libc::FILE => fizzle_freopen(ctx) {
        // SAFETY: caller guarantees `pathaname` and `mode` point to a null-terminated string
        let path_cstr = unsafe { CStr::from_ptr(pathname) };
        let mode_cstr = unsafe { CStr::from_ptr(mode) };
        crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> ...", path_cstr, mode_cstr, stream);

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("freopen() passed null `stream` parameter")
        };

        let fd = libc::open(pathname, stream_mode.flags.bits());
        if fd < 0 {
            // TODO: need to finish implementing
            crate::strace!("freopen(pathname={:?}, mode={:?}, stream={:?}) -> NULL", path_cstr, mode_cstr, stream);
            return ptr::null_mut()
        }

        match Scheduler::handle_event(&mut ctx, StreamCreateEvent::new(FileStreamSource::Descriptor(fd), stream_mode, Some(file_ptr))) {
            Ok(mut file_ptr) => {
                crate::strace!("freopen(fd={}, mode={:?}) -> {:?}", fd, mode_cstr, file_ptr);
                file_ptr.as_raw()
            }
            Err(e) => {
                crate::strace!("fdopen(fd={}, mode={:?}) -> NULL ({})", fd, mode_cstr, e);
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

        let Some(stream_mode) = FileStreamMode::from_cstr(mode_cstr) else {
            crate::strace!("fopen64(pathname={:?}, mode={:?}) -> NULL (EINVAL)", path_cstr, mode_cstr);
            Errno::EINVAL.set_errno();
            return ptr::null_mut()
        };

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("freopen64() passed null `stream` parameter")
        };

        let fd = libc::open(pathname, stream_mode.flags.bits());
        if fd < 0 {
            // TODO: need to finish implementing
            crate::strace!("freopen64(pathname={:?}, mode={:?}, stream={:?}) -> NULL", path_cstr, mode_cstr, stream);
            return ptr::null_mut()
        }

        match Scheduler::handle_event(&mut ctx, StreamCreateEvent::new(FileStreamSource::Descriptor(fd), stream_mode, Some(file_ptr))) {
            Ok(mut file_ptr) => {
                crate::strace!("freopen64(fd={}, mode={:?}) -> {:?}", fd, mode_cstr, file_ptr);
                file_ptr.as_raw()
            }
            Err(e) => {
                crate::strace!("fdopen64(fd={}, mode={:?}) -> NULL ({})", fd, mode_cstr, e);
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
        crate::strace!("fclose(stream={:?}) -> ...", stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fclose(stream={:?}) -> -1 (EINVAL)", stream);
            Errno::EINVAL.set_errno();
            return -1
        };

        if let Err(e) = Scheduler::handle_event(&mut ctx, StreamFlushEvent::new(Some(stream_ptr), false)) {
            crate::strace!("fclose(stream={:?}) -> -1 ({})", stream, e);
            e.set_errno();
            return -1
        }

        match Scheduler::handle_event(&mut ctx, StreamCloseEvent::new(&stream_ptr)) {
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
        crate::strace!("fflush(stream={:?}) -> ...", stream);

        let stream_ptr_opt = FilePtr::from_raw(stream);

        match Scheduler::handle_event(&mut ctx, StreamFlushEvent::new(stream_ptr_opt, false)) {
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
    unsafe fn fflush_unlocked(stream: *mut libc::FILE) -> libc::c_int => fizzle_fflush_unlocked(ctx) {
        crate::strace!("fflush_unlocked(stream={:?}) -> ...", stream);

        let stream_ptr_opt = FilePtr::from_raw(stream);

        match Scheduler::handle_event(&mut ctx, StreamFlushEvent::new(stream_ptr_opt, true)) {
            Ok(()) => {
                crate::strace!("fflush_unlocked(stream={:?}) -> 0", stream);
                0
            }
            Err(e) => {
                crate::strace!("fflush_unlocked(stream={:?}) -> -1 ({})", stream, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fileno(stream: *mut libc::FILE) -> libc::c_int => fizzle_fileno(ctx) {
        crate::strace!("fileno(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fileno(stream={:?}) -> -1 (EINVAL)", stream);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, StreamDescriptorEvent::new(file_ptr, false)) {
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
    unsafe fn fileno_unlocked(stream: *mut libc::FILE) -> libc::c_int => fizzle_fileno_unlocked(ctx) {
        crate::strace!("fileno_unlocked(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fileno_unlocked(stream={:?}) -> -1 (EINVAL)", stream);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, StreamDescriptorEvent::new(file_ptr, true)) {
            Ok(fd) => {
                crate::strace!("fileno_unlocked(stream={:?}) -> {}", stream, fd);
                fd
            }
            Err(e) => {
                crate::strace!("fileno_unlocked(stream={:?}) -> -1 ({})", stream, e);
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

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 (EINVAL)", ptr, size, nmemb, stream);
            Errno::EINVAL.set_errno();
            return 0
        };

        let buf = slice::from_raw_parts(ptr.cast::<u8>(), size * nmemb);

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, buf, size, false)) {
            Ok(()) => {
                crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, nmemb);
                nmemb
            }
            Err(written) => {
                crate::strace!("fwrite(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, written);
                written
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fwrite_unlocked(
        ptr: *const libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwrite_unlocked(ctx) {
        crate::strace!("fwrite_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> ...", ptr, size, nmemb, stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fwrite_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 (EINVAL)", ptr, size, nmemb, stream);
            Errno::EINVAL.set_errno();
            return 0
        };

        let buf = slice::from_raw_parts(ptr.cast::<u8>(), size * nmemb);

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, buf, size, true)) {
            Ok(()) => {
                crate::strace!("fwrite_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, nmemb);
                nmemb
            }
            Err(written) => {
                crate::strace!("fwrite_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, written);
                written
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

        match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, buf, size, false, false)) {
            Ok(_) => {
                crate::strace!("fread(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, nmemb);
                nmemb
            }
            Err(written) => {
                crate::strace!("fread(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, written);
                written
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fread_unlocked(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fread_unlocked(ctx) {
        crate::strace!("fread_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> ...", ptr, size, nmemb, stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            crate::strace!("fread_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> -1 (EINVAL)", ptr, size, nmemb, stream);
            Errno::EINVAL.set_errno();
            return 0
        };

        let buf = slice::from_raw_parts_mut(ptr.cast::<u8>(), size * nmemb);

        match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, buf, size, false, false)) {
            Ok(_) => {
                crate::strace!("fread_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, nmemb);
                nmemb
            }
            Err(written) => {
                crate::strace!("fread_unlocked(ptr={:?}, size={}, nmemb={}, stream={:?}) -> {}", ptr, size, nmemb, stream, written);
                written
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetc(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fgetc(ctx) {
        crate::strace!("fgetc(stream={:?}) -> ...", stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fgetc()")
        };

        let mut buf = [0u8; 1];

        match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf, 1, false, false)) {
            Ok(_) => {
                crate::strace!("fgetc(stream={:?}) -> {}", stream, buf[0]);
                buf[0] as libc::c_int
            }
            Err(_written) => {
                crate::strace!("fgetc(stream={:?}) -> EOF", stream);
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetc_unlocked(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fgetc_unlocked(ctx) {
        crate::strace!("fgetc_unlocked(stream={:?}) -> ...", stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fgetc_unlocked()")
        };

        let mut buf = [0u8; 1];

        match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf, 1, true, false)) {
            Ok(_) => {
                crate::strace!("fgetc_unlocked(stream={:?}) -> {}", stream, buf[0]);
                buf[0] as libc::c_int
            }
            Err(_written) => {
                crate::strace!("fgetc_unlocked(stream={:?}) -> EOF", stream);
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fgets(
        s: *mut libc::c_char,
        size: libc::c_int,
        stream: *mut libc::FILE
    ) -> *mut libc::c_char => fizzle_fgets(ctx) {
        crate::strace!("fgets(s={:?}, size={}, stream={:?}) -> ...", s, size, stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fgets()")
        };

        if size < 1 {
            panic!("invalid size passed to fgets()")
        }

        let buf = slice::from_raw_parts_mut(s.cast::<u8>(), size as usize);
        let buf_len = buf.len().saturating_sub(1);

        match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[..buf_len], 1, false, true)) {
            Ok(written) => {
                if buf[written - 1] != b'\n' {
                    buf[buf_len] = b'\0';
                }

                crate::strace!("fgets(s={:?}, size={}, stream={:?}) -> {:?} ({:?})", s, size, stream, s, CStr::from_bytes_with_nul_unchecked(buf));
                s
            }
            Err(written) => {
                let ret = if written == 0 {
                    ptr::null_mut()
                } else {
                    s
                };

                buf[written] = b'\0';

                crate::strace!("fgets(s={:?}, size={}, stream={:?}) -> {:?} ({:?})", s, size, stream, ret, CStr::from_bytes_with_nul_unchecked(buf));
                ret
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fgets_unlocked(
        s: *mut libc::c_char,
        size: libc::c_int,
        stream: *mut libc::FILE
    ) -> *mut libc::c_char => fizzle_fgets_unlocked(ctx) {
        crate::strace!("fgets_unlocked(s={:?}, size={}, stream={:?}) -> ...", s, size, stream);

        let Some(stream_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fgets_unlocked()")
        };

        if size < 1 {
            panic!("invalid size passed to fgets_unlocked()")
        }

        let buf = slice::from_raw_parts_mut(s.cast::<u8>(), size as usize);
        let buf_len = buf.len() - 1;

        match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[..buf_len], 1, false, true)) {
            Ok(written) => {
                if buf[written - 1] != b'\n' {
                    buf[buf_len] = b'\0';
                }

                crate::strace!("fgets_unlocked(s={:?}, size={}, stream={:?}) -> {:?}", s, size, stream, s);
                s
            }
            Err(written) => {
                let ret = if written == 0 || Errno::get_errno() != Errno::SUCCESS {
                    ptr::null_mut()
                } else {
                    s
                };

                buf[written] = b'\0';

                crate::strace!("fgets_unlocked(s={:?}, size={}, stream={:?}) -> {:?}", s, size, stream, ret);
                ret
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn ungetc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ungetc(ctx) {
        crate::strace!("ungetc(c={:?}, stream={:?}) -> ...", c, stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to ungetc()")
        };

        match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(file_ptr, c as u8, true)) {
            Ok(()) => {
                crate::strace!("ungetc(c={:?}, stream={:?}) -> {:?}", c, stream, c);
                return c
            }
            Err(()) => {
                crate::strace!("ungetc(c={:?}, stream={:?}) -> EOF", c, stream);
                return libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fputc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputc(ctx) {
        crate::strace!("fputc(c={}, stream={:?}) -> ...", c, stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fputc()")
        };

        let buf = [c as u8];

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, &buf, 1, false)) {
            Ok(()) => {
                crate::strace!("fputc(c={:?}, stream={:?}) -> {}", c, stream, c);
                c
            }
            Err(_written) => {
                crate::strace!("fputc(c={:?}, stream={:?}) -> EOF", c, stream);
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fputc_unlocked(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputc_unlocked(ctx) {
        crate::strace!("fputc_unlocked(c={}, stream={:?}) -> ...", c, stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fputc_unlocked()")
        };

        let buf = [c as u8];

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, &buf, 1, true)) {
            Ok(()) => {
                crate::strace!("fputc_unlocked(c={:?}, stream={:?}) -> {}", c, stream, c);
                c
            }
            Err(_written) => {
                crate::strace!("fputc_unlocked(c={:?}, stream={:?}) -> EOF", c, stream);
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

        let file_ptr = FilePtr::from_raw(crate::stdout).unwrap();
        let buf = [c as u8];

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, &buf, 1, false)) {
            Ok(()) => {
                crate::strace!("putchar(c={:?}) -> {}", c, c);
                c
            }
            Err(_written) => {
                crate::strace!("putchar(c={:?}) -> EOF", c);
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn putchar_unlocked(
        c: libc::c_int
    ) -> libc::c_int => fizzle_putchar_unlocked(ctx) {
        crate::strace!("putchar_unlocked(c={}) -> ...", c);

        let file_ptr = FilePtr::from_raw(crate::stdout).unwrap();
        let buf = [c as u8];

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, &buf, 1, true)) {
            Ok(()) => {
                crate::strace!("putchar_unlocked(c={:?}) -> {}", c, c);
                c
            }
            Err(_written) => {
                crate::strace!("putchar_unlocked(c={:?}) -> EOF", c);
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

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fputs()")
        };

        let buf = CStr::from_ptr(s).to_bytes();

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, buf, 1, false)) {
            Ok(()) => {
                crate::strace!("fputs(s={:?}, stream={:?}) -> 0", s, stream);
                0
            }
            Err(_written) => {
                crate::strace!("fputs(s={:?}, stream={:?}) -> EOF", s, stream);
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fputs_unlocked(
        s: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputs_unlocked(ctx) {
        crate::strace!("fputs_unlocked(s={:?}, stream={:?}) -> ...", s, stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fputs()")
        };

        let buf = CStr::from_ptr(s).to_bytes();

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, buf, 1, true)) {
            Ok(()) => {
                crate::strace!("fputs_unlocked(s={:?}, stream={:?}) -> 0", s, stream);
                0
            }
            Err(_written) => {
                crate::strace!("fputs_unlocked(s={:?}, stream={:?}) -> EOF", s, stream);
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

        let file_ptr = FilePtr::from_raw(crate::stdout).unwrap();
        let buf = CStr::from_ptr(s).to_bytes();

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, buf, 1, false)) {
            Ok(()) => {
                crate::strace!("puts(s={:?}) -> 0", s);
                0
            }
            Err(_written) => {
                crate::strace!("puts(s={:?}) -> EOF", s);
                libc::EOF
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn puts_unlocked(
        s: *const libc::c_char
    ) -> libc::c_int => fizzle_puts_unlocked(ctx) {
        crate::strace!("puts_unlocked(s={:?}) -> ...", s);

        let file_ptr = FilePtr::from_raw(crate::stdout).unwrap();
        let buf = CStr::from_ptr(s).to_bytes();

        match Scheduler::handle_event(&mut ctx, StreamWriteEvent::new(file_ptr, buf, 1, true)) {
            Ok(()) => {
                crate::strace!("puts_unlocked(s={:?}) -> 0", s);
                0
            }
            Err(_written) => {
                crate::strace!("puts_unlocked(s={:?}) -> EOF", s);
                libc::EOF
            }
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct CookieIoFunctions {
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
        cookie: *mut libc::c_void,
        mode: *const libc::c_char,
        io_funcs: CookieIoFunctions
    ) -> *mut libc::FILE => fizzle_fopencookie(_ctx) {
        unimplemented!("fopencookie")
    }
}

hook_macros::hook! {
    unsafe fn fseek(
        stream: *mut libc::FILE,
        offset: libc::c_long,
        whence: libc::c_int
    ) -> libc::c_int => fizzle_fseek(ctx) {
        crate::strace!("fseek(stream={:?}, offset={}, whence={}) -> ...", stream, offset, whence);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fseek()")
        };

        let position = match whence {
            libc::SEEK_SET => SeekPosition::Start,
            libc::SEEK_CUR => SeekPosition::Current,
            libc::SEEK_END => SeekPosition::End,
            _ => {
                crate::strace!("fseek(stream={:?}, offset={}, whence={}) -> -1 (EINVAL)", stream, offset, whence);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, StreamFlushEvent::new(Some(file_ptr), false)) {
            Ok(()) => (),
            Err(_) => {
                let e = Errno::get_errno();
                log::warn!("flush during fseek() failed: {}", e);
                crate::strace!("fseek(stream={:?}, offset={}, whence={}) -> -1 (EINVAL)", stream, offset, whence);
                e.set_errno();
                return -1
            }
        }

        match Scheduler::handle_event(&mut ctx, StreamSeekEvent::new(file_ptr, position, offset, false)) {
            Ok(_) => {
                crate::strace!("fseek(stream={:?}, offset={}, whence={}) -> 0", stream, offset, whence);
                0
            }
            Err(e) => {
                crate::strace!("fseek(stream={:?}, offset={}, whence={}) -> -1 ({})", stream, offset, whence, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn ftell(
        stream: *mut libc::FILE
    ) -> libc::c_long => fizzle_ftell(ctx) {
        crate::strace!("ftell(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to ftell()")
        };

        match Scheduler::handle_event(&mut ctx, StreamTellEvent::new(file_ptr)) {
            Ok(offset) => {
                crate::strace!("ftell(stream={:?}) -> {}", stream, offset);
                offset as libc::c_long
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn rewind(
        stream: *mut libc::FILE
    ) => fizzle_rewind(ctx) {
        crate::strace!("rewind(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to rewind()")
        };

        match Scheduler::handle_event(&mut ctx, StreamFlushEvent::new(Some(file_ptr), false)) {
            Ok(()) => (),
            Err(_) => {
                let e = Errno::get_errno();
                log::warn!("flush during rewind() failed: {}", e);
                return
            }
        }

        match Scheduler::handle_event(&mut ctx, StreamSeekEvent::new(file_ptr, SeekPosition::Start, 0, true)) {
            Ok(_) => {
                crate::strace!("rewind(stream={:?}) -> ()", stream);
            }
            Err(e) => {
                // TODO: rewind is an infallible call; should we define a new flush function
                // that unconditionally resets buffers to accomodate this?
                log::warn!("fseek during rewind() failed: {}", e);
                crate::strace!("rewind(stream={:?}) -> ()", stream);
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetpos(
        stream: *mut libc::FILE,
        pos: *mut i64 // was libc::fpos_t, but that isn't defined...
    ) -> libc::c_int => fizzle_fgetpos(ctx) {
        crate::strace!("fgetpos(stream={:?}, pos={:?}) -> ...", stream, pos);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fgetpos()")
        };

        match Scheduler::handle_event(&mut ctx, StreamTellEvent::new(file_ptr)) {
            Ok(offset) => {
                unsafe {
                    *pos = offset as i64;
                }
                crate::strace!("fgetpos(stream={:?}, pos={:?}) -> 0", stream, pos);
                0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn fsetpos(
        stream: *mut libc::FILE,
        pos: *const i64 // was libc::fpos_t
    ) -> libc::c_int => fizzle_fsetpos(ctx) {
        crate::strace!("fsetpos(stream={:?}, pos={:?}) -> ...", stream, pos);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fsetpos()")
        };

        match Scheduler::handle_event(&mut ctx, StreamFlushEvent::new(Some(file_ptr), false)) {
            Ok(()) => (),
            Err(_) => {
                let e = Errno::get_errno();
                log::warn!("flush during fsetpos() failed: {}", e);
                crate::strace!("fsetpos(stream={:?}, pos={:?}) -> -1 ({})", stream, pos, e);
                e.set_errno();
                return -1
            }
        }

        let offset = unsafe { *pos };

        match Scheduler::handle_event(&mut ctx, StreamSeekEvent::new(file_ptr, SeekPosition::Start, offset, false)) {
            Ok(_) => {
                crate::strace!("fsetpos(stream={:?}, pos={:?}) -> 0", stream, pos);
                0
            }
            Err(e) => {
                crate::strace!("fsetpos(stream={:?}, pos={:?}) -> -1 ({})", stream, pos, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn clearerr(
        stream: *mut libc::FILE
    ) => fizzle_clearerr(ctx) {
        crate::strace!("clearerr(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to clearerr()")
        };

        match Scheduler::handle_event(&mut ctx, StreamClearErrorEvent::new(file_ptr, false)) {
            Ok(()) => {
                crate::strace!("clearerr(stream={:?}) -> ()", stream);
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn clearerr_unlocked(
        stream: *mut libc::FILE
    ) => fizzle_clearerr_unlocked(ctx) {
        crate::strace!("clearerr_unlocked(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to clearerr_unlocked()")
        };

        match Scheduler::handle_event(&mut ctx, StreamClearErrorEvent::new(file_ptr, false)) {
            Ok(()) => {
                crate::strace!("clearerr_unlocked(stream={:?}) -> ()", stream);
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn feof(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_feof(ctx) {
        crate::strace!("feof(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to feof()")
        };

        match Scheduler::handle_event(&mut ctx, StreamEofEvent::new(file_ptr, false)) {
            Ok(true) => {
                crate::strace!("feof(stream={:?}) -> -1", stream);
                return -1
            }
            Ok(false) => {
                crate::strace!("feof(stream={:?}) -> 0", stream);
                return 0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn feof_unlocked(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_feof_unlocked(ctx) {
        crate::strace!("feof_unlocked(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to feof()")
        };

        match Scheduler::handle_event(&mut ctx, StreamEofEvent::new(file_ptr, true)) {
            Ok(true) => {
                crate::strace!("feof_unlocked(stream={:?}) -> -1", stream);
                return -1
            }
            Ok(false) => {
                crate::strace!("feof_unlocked(stream={:?}) -> 0", stream);
                return 0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn ferror(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ferror(ctx) {
        crate::strace!("ferror(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to ferror()")
        };

        match Scheduler::handle_event(&mut ctx, StreamErrorEvent::new(file_ptr, false)) {
            Ok(true) => {
                crate::strace!("ferror(stream={:?}) -> -1", stream);
                return -1
            }
            Ok(false) => {
                crate::strace!("ferror(stream={:?}) -> 0", stream);
                return 0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn ferror_unlocked(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ferror_unlocked(ctx) {
        crate::strace!("ferror_unlocked(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to ferror()")
        };

        match Scheduler::handle_event(&mut ctx, StreamErrorEvent::new(file_ptr, true)) {
            Ok(true) => {
                crate::strace!("ferror_unlocked(stream={:?}) -> -1", stream);
                return -1
            }
            Ok(false) => {
                crate::strace!("ferror_unlocked(stream={:?}) -> 0", stream);
                return 0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn flockfile(
        stream: *mut libc::FILE
    ) => fizzle_flockfile(ctx) {
        crate::strace!("flockfile(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to flockfile()")
        };

        match Scheduler::handle_event(&mut ctx, StreamLockEvent::new(file_ptr)) {
            Ok(()) => {
                crate::strace!("flockfile(stream={:?}) -> ()", stream);
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn funlockfile(
        stream: *mut libc::FILE
    ) => fizzle_funlockfile(ctx) {
        crate::strace!("funlockfile(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to funlockfile()")
        };

        match Scheduler::handle_event(&mut ctx, StreamUnlockEvent::new(file_ptr)) {
            Ok(()) => {
                crate::strace!("funlockfile(stream={:?}) -> ()", stream);
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn ftrylockfile(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ftrylockfile(ctx) {
        crate::strace!("ftrylockfile(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to ftrylockfile()")
        };

        match Scheduler::handle_event(&mut ctx, StreamTryLockEvent::new(file_ptr)) {
            Ok(()) => {
                crate::strace!("ftrylockfile(stream={:?}) -> 0", stream);
                return 0
            }
            Err(()) => {
                crate::strace!("ftrylockfile(stream={:?}) -> -1", stream);
                return -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fpurge(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fpurge(ctx) {
        crate::strace!("fpurge(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to fpurge()")
        };

        match Scheduler::handle_event(&mut ctx, StreamPurgeEvent::new(file_ptr)) {
            Ok(()) => {
                crate::strace!("fpurge(stream={:?}) -> 0", stream);
                0
            },
            Err(e) => {
                crate::strace!("fpurge(stream={:?}) -> -1 ({})", stream, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn __fbufsize(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fbufsize(ctx) {
        crate::strace!("__fbufsize(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __fbufsize()")
        };

        match Scheduler::handle_event(&mut ctx, StreamBufSizeEvent::new(file_ptr)) {
            Ok(len) => {
                crate::strace!("__fbufsize(stream={:?}) -> {}", stream, len);
                len
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __fpending(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fpending(ctx) {
        crate::strace!("__fpending(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __fpending()")
        };

        match Scheduler::handle_event(&mut ctx, StreamPendingEvent::new(file_ptr)) {
            Ok(pending) => {
                crate::strace!("__fpending(stream={:?}) -> {}", stream, pending);
                pending
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __flbf(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_flbf(ctx) {
        crate::strace!("__flbf(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __flbf()")
        };

        match Scheduler::handle_event(&mut ctx, StreamLineBufferedEvent::new(file_ptr)) {
            Ok(true) => {
                crate::strace!("__flbf(stream={:?}) -> 1", stream);
                1
            },
            Ok(false) => {
                crate::strace!("__flbf(stream={:?}) -> 0", stream);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __freadable(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_freadable(ctx) {
        crate::strace!("__freadable(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __freadable()")
        };

        match Scheduler::handle_event(&mut ctx, StreamReadableEvent::new(file_ptr)) {
            Ok(true) => {
                crate::strace!("__freadable(stream={:?}) -> 1", stream);
                1
            },
            Ok(false) => {
                crate::strace!("__freadable(stream={:?}) -> 0", stream);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __fwritable(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fwritable(ctx) {
        crate::strace!("__fwritable(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __fwritable()")
        };

        match Scheduler::handle_event(&mut ctx, StreamWritableEvent::new(file_ptr)) {
            Ok(true) => {
                crate::strace!("__fwritable(stream={:?}) -> 1", stream);
                1
            },
            Ok(false) => {
                crate::strace!("__fwritable(stream={:?}) -> 0", stream);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __freading(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_freading(ctx) {
        crate::strace!("__freading(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __freading()")
        };

        match Scheduler::handle_event(&mut ctx, StreamReadingEvent::new(file_ptr)) {
            Ok(true) => {
                crate::strace!("__freading(stream={:?}) -> 1", stream);
                1
            },
            Ok(false) => {
                crate::strace!("__freading(stream={:?}) -> 0", stream);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __fwriting(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fwriting(ctx) {
        crate::strace!("__fwriting(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __fwriting()")
        };

        match Scheduler::handle_event(&mut ctx, StreamWritingEvent::new(file_ptr)) {
            Ok(true) => {
                crate::strace!("__fwriting(stream={:?}) -> 1", stream);
                1
            },
            Ok(false) => {
                crate::strace!("__fwriting(stream={:?}) -> 0", stream);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn __fsetlocking(
        stream: *mut libc::FILE,
        lock_type: libc::c_int
    ) -> libc::c_int => fizzle_fsetlocking(ctx) {
        let locking = match lock_type {
            FSETLOCKING_QUERY => None,
            FSETLOCKING_BYCALLER => Some(false),
            FSETLOCKING_INTERNAL => Some(true),
            _ => panic!("invalid value passed to __fsetlocking()"),
        };

        crate::strace!("__fsetlocking(stream={:?}, lock_type={:?}) -> ...", stream, locking);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __fsetlocking()")
        };

        match Scheduler::handle_event(&mut ctx, StreamSetLockingEvent::new(file_ptr, locking)) {
            Ok(true) => {
                crate::strace!("__fsetlocking(stream={:?}, lock_type={:?}) -> FSETLOCKING_INTERNAL", stream, locking);
                FSETLOCKING_INTERNAL
            },
            Ok(false) => {
                crate::strace!("__fsetlocking(stream={:?}, lock_type={:?}) -> FSETLOCKING_BYCALLER", stream, locking);
                FSETLOCKING_BYCALLER
            },
            Err(()) => unreachable!(),
        }
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
    ) => fizzle_fpurge2(ctx) {
        crate::strace!("__fpurge(stream={:?}) -> ...", stream);

        let Some(file_ptr) = FilePtr::from_raw(stream) else {
            panic!("invalid FILE* pointer passed to __fpurge()")
        };

        match Scheduler::handle_event(&mut ctx, StreamPurgeEvent::new(file_ptr)) {
            Ok(()) => {
                crate::strace!("__fpurge(stream={:?}) -> ()", stream);
            },
            Err(_) => panic!("error during __fpurge()"),
        }
    }
}
