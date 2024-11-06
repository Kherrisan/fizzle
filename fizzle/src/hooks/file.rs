use std::ffi::{CStr, CString};
use std::ptr;

use fizzle_common::path::FilePath;

use crate::backend::FileBackend;
use crate::handlers::descriptor::{DescriptorId, DescriptorInfo, FdResource};
use crate::handlers::file::{FileError, FileObject, FilePtr};
use crate::hook_macros;

hook_macros::hook! {
    unsafe fn fanotify_init(
        _flags: libc::c_uint,
        _event_f_flags: libc::c_uint
    ) => fizzle_fanotify_init(_ctx) {
        unimplemented!("fanotify_init()")
    }
}

hook_macros::hook! {
    unsafe fn inotify_init1(
        _flags: libc::c_int
    ) => fizzle_inotify_init1(_ctx) {
        unimplemented!("inotify_init1()")
    }
}

hook_macros::hook! {
    unsafe fn inotify_init() => fizzle_inotify_init(_ctx) {
        unimplemented!("inotify_init()")
    }
}

hook_macros::hook! {
    unsafe fn fdopen(
        fd: libc::c_int,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fdopen(ctx) {
        let mut state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        let Some(fd_info) = state.local.fds.get(&descriptor_id) else {
            log::warn!("file descriptor {} for fdopen() not in Fizzle local fds", fd);

            let p = hook_macros::real!(fdopen)(fd, mode);

            if p.is_null() {
                log::debug!("fdopen({}, {:?}) -> NULL (errno {})", fd, CStr::from_ptr(mode), *libc::__errno_location());
            } else {
                log::debug!("fdopen({}, {:?}) -> {:?}", fd, CStr::from_ptr(mode), p);
            }

            return p
        };

        let file = match fd_info.resource {
            FdResource::File(_) => crate::unique_mem_create() as *mut libc::FILE,
            _ => {
                log::debug!("fdopen() called with unusual (non-file) file descriptor");
                crate::unique_mem_create() as *mut libc::FILE
            },
        };

        // TODO: parse and use `mode`
        let file_id = FilePtr::from(file);

        let None = state.local.file_objs.insert(file_id, FileObject::new(fd)) else {
            log::error!("Multiple FILE* objects opened for one file descriptor");
            panic!()
        };

        log::debug!("fdopen({}, {:?} -> {:?}", fd, CStr::from_ptr(mode), file);

        file
    }
}

hook_macros::hook! {
    unsafe fn fopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fopen(ctx) {
        let mut state = ctx.acquire();

        let path = CStr::from_ptr(pathname).to_str().unwrap();

        // NOTE: temporary workaround for programs that check thread count via the status
        if &path[..6] == "/proc/" && &path[path.len() - 7..] == "/status" {
            let tmp_path = format!("/tmp/{}-status", libc::getpid());

            let mut data = std::fs::read_to_string(path).unwrap();

            if let Some(mut offset) = data.find("Threads:") {
                offset += "Threads:".len();
                let data_slice = &mut data.as_bytes_mut();

                while data_slice[offset] != b'\n' {
                    data_slice[offset] = b' ';
                    offset += 1;
                }
                data_slice[offset - 1] = b'1';
            }

            std::fs::write(&tmp_path, data).unwrap();
            let tmp_cstr = CString::new(tmp_path.as_str()).unwrap();
            return hook_macros::real!(fopen)(tmp_cstr.as_ptr(), mode)
        }

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            log::warn!("fopen() received malformed or oversized filepath \"{}\"", path);
            log::debug!("fopen(pathname={:?}, mode={:?}) -> NULL (ENAMETOOLONG)", CStr::from_ptr(pathname), CStr::from_ptr(mode));
            *libc::__errno_location() = libc::ENAMETOOLONG; // TODO: split apart errors from backpathing too much (/../) vs too-long errors
            return ptr::null_mut() as *mut libc::FILE
        };

        let path = if relative_path.is_absolute() {
            relative_path

        } else {
            let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
                log::warn!("fopen() filepath oversized when converted to absolute path: \"{}\"", path);
                log::debug!("fopen(pathname={:?}, mode={:?}) -> NULL (ENAMETOOLONG)", CStr::from_ptr(pathname), CStr::from_ptr(mode));
                *libc::__errno_location() = libc::ENAMETOOLONG;
                return ptr::null_mut()
            };

            path
        };

        let fd = if let Some(file_id) = state.global.file_paths.get(&path) {
            let file_id = file_id.clone();
            let fd = crate::create_descriptor();
            state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
                close_on_exec: false,
                nonblocking: false,
                is_passthrough: false,
                resource: FdResource::File(file_id),
            }).unwrap();

            fd

        } else {
            let fd = libc::open(pathname, 0, 0); // TODO: account for mode here
            if fd >= 0 {
                let file_id = state.global.files.allocate(FileBackend::Passthrough).unwrap();

                state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
                    close_on_exec: false,
                    nonblocking: false,
                    is_passthrough: true,
                    resource: FdResource::File(file_id),
                }).unwrap();
            }

            fd
        };

        let file = crate::unique_mem_create() as *mut libc::FILE;

        let None = state.local.file_objs.insert(FilePtr::from(file), FileObject::new(fd)) else {
            log::error!("non-unique virtual file handle allocated in fopen()");
            panic!()
        };

        log::debug!("fopen(pathname={:?}, mode={:?}) -> {:?}", CStr::from_ptr(pathname), CStr::from_ptr(mode), file);

        file
    }
}

hook_macros::hook! {
    unsafe fn freopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> *mut libc::FILE => fizzle_freopen(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for freopen() not in Fizzle local file streams", stream);

            let p = hook_macros::real!(freopen)(pathname, mode, stream);

            if p.is_null() {
                log::debug!("freopen({:?}, {:?}, {:?}) -> NULL (errno {})", CStr::from_ptr(pathname), CStr::from_ptr(mode), stream, *libc::__errno_location());
            } else {
                log::debug!("freopen({:?}, {:?}, {:?}) -> {:?}", CStr::from_ptr(pathname), CStr::from_ptr(mode), stream, p);
            }

            return p
        };

        panic!("freopen() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn fclose(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fclose(ctx) {
        if stream.is_null() {
            let state = ctx.acquire();
            let keys: Vec<FilePtr> = state.local.file_objs.keys().cloned().collect();
            drop(state);

            for key in keys {
                key.close(&mut ctx).unwrap();
            }
            0

        } else {
            let file_ptr = FilePtr::from(stream);

            match file_ptr.close(&mut ctx) {
                Ok(_) => 0,
                Err(FileError::InvalidPtr) => {
                    log::warn!("FILE* {:?} for fclose() not in Fizzle state", stream);

                    if libc::fclose(stream) == 0 {
                        log::debug!("fclose(stream={:?}) -> 0", stream);
                        0
                    } else {
                        log::debug!("fclose(stream={:?}) -> -1 (errno {})", stream, *libc::__errno_location());
                        -1
                    }
                }
                Err(e) => {
                    *libc::__errno_location() = e.as_os_error();
                    log::debug!("fclose(stream={:?}) -> -1 ({})", stream, e);
                    -1
                }
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fileno(stream: *mut libc::FILE) -> libc::c_int => fizzle_fileno(ctx) {
        let state = ctx.acquire();

        match state.local.file_objs.get(&FilePtr::from(stream)) {
            Some(file) => file.fd,
            None => hook_macros::real!(fileno)(stream),
        }
    }
}

hook_macros::hook! {
    unsafe fn fflush(stream: *mut libc::FILE) -> libc::c_int => fizzle_fflush(ctx) {
        let state = ctx.acquire();

        match state.local.file_objs.get(&FilePtr::from(stream)) {
            Some(_) => 0,
            None => hook_macros::real!(fflush)(stream),
        }
    }
}

hook_macros::hook! {
    unsafe fn fwrite(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwrite(ctx) {
        let mut state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get_mut(&file_id) {
            Some(_fd) => unimplemented!("fwrite()"),
            None => hook_macros::real!(fwrite)(ptr, size, nmemb, stream),
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
        let mut state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get_mut(&file_id) {
            Some(_fd) => unimplemented!("fread()"),
            None => hook_macros::real!(fread)(ptr, size, nmemb, stream),
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetc(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fgetc(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for fgetc() not in Fizzle local file streams", stream);

            let c = hook_macros::real!(fgetc)(stream);

            log::debug!("fgetc({:?}) -> {}", stream, c);

            return c
        };

        unimplemented!("fgetc()")
    }
}

hook_macros::hook! {
    unsafe fn getc(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_getc(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for getc() not in Fizzle local file streams", stream);

            // NOTE: this is hooked for `fgetc()` rather than `getc()` as the latter may be a macro
            let c = hook_macros::real!(fgetc)(stream);

            log::debug!("getc({:?}) -> {}", stream, c);

            return c
        };

        panic!("getc() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn ungetc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ungetc(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for ungetc() not in Fizzle local file streams", stream);

            let ret = hook_macros::real!(ungetc)(c, stream);

            log::debug!("ungetc({}, {:?}) -> {}", c, stream, ret);

            return ret
        };

        panic!("ungetc() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn fputc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputc(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for fputc() not in Fizzle local file streams", stream);

            let ret = hook_macros::real!(fputc)(c, stream);

            log::debug!("fputc({}, {:?}) -> {}", c, stream, ret);

            return ret
        };

        panic!("fputc() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn putc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_putc(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for putc() not in Fizzle local file streams", stream);

            let ret = hook_macros::real!(fputc)(c, stream);

            log::debug!("putc({}, {:?}) -> {}", c, stream, ret);

            return ret
        };

        panic!("putc() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn putchar(
        _c: libc::c_int
    ) -> libc::c_int => fizzle_putchar(_ctx) {
        panic!("putchar() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn fputs(
        s: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputs(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);

        let Some(_file) = state.local.file_objs.get(&file_id) else {
            log::warn!("file stream {:?} for fputs() not in Fizzle local file streams", stream);

            let ret = hook_macros::real!(fputs)(s, stream);

            log::debug!("fputs({:?}, {:?}) -> {}", CStr::from_ptr(s), stream, ret);

            return ret
        };

        unimplemented!("fputs()")
    }
}

hook_macros::hook! {
    unsafe fn puts(
        _s: *const libc::c_char
    ) -> libc::c_int => fizzle_puts(_ctx) {
        unimplemented!("puts()")
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
    unsafe fn fseek(
        stream: *mut libc::FILE,
        offset: libc::c_long,
        whence: libc::c_int
    ) -> libc::c_int => fizzle_fseek(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        if state.local.file_objs.contains_key(&file_id) {
            unimplemented!("fseek()")
        } else {
            hook_macros::real!(fseek)(stream, offset, whence)
        }
    }
}

hook_macros::hook! {
    unsafe fn ftell(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ftell(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        if state.local.file_objs.contains_key(&file_id) {
            unimplemented!("ftell()")
        } else {
            hook_macros::real!(ftell)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn frewind(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_frewind(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        if state.local.file_objs.contains_key(&file_id) {
            unimplemented!("frewind()")
        } else {
            hook_macros::real!(frewind)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetpos(
        stream: *mut libc::FILE,
        pos: *mut libc::fpos_t
    ) -> libc::c_int => fizzle_fgetpos(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        if state.local.file_objs.contains_key(&file_id) {
            unimplemented!("fgetpos()")
        } else {
            hook_macros::real!(fgetpos)(stream, pos)
        }
    }
}

hook_macros::hook! {
    unsafe fn fsetpos(
        stream: *mut libc::FILE,
        pos: *const libc::fpos_t
    ) -> libc::c_int => fizzle_fsetpos(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        if state.local.file_objs.contains_key(&file_id) {
            unimplemented!("fsetpos()")
        } else {
            hook_macros::real!(fsetpos)(stream, pos)
        }
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
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__fbufsize()")
            }
            None => hook_macros::real!(__fbufsize)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __fpending(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fpending(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__fpending()")
            }
            None => hook_macros::real!(__fpending)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __flbf(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_flbf(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__flbf()")
            }
            None => hook_macros::real!(__flbf)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __freadable(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_freadable(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__freadable()");
            }
            None => hook_macros::real!(__freadable)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __fwritable(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fwritable(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__fwritable()")
            }
            None => hook_macros::real!(__fwritable)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __freading(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_freading(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__freading()")
            }
            None => hook_macros::real!(__freading)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __fwriting(
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwriting(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__fwriting()")
            }
            None => hook_macros::real!(__fwriting)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn __fsetlocking(
        stream: *mut libc::FILE,
        lock_type: libc::c_int
    ) -> libc::c_int => fizzle_fsetlocking(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__fsetlocking()")
            }
            None => hook_macros::real!(__fsetlocking)(stream, lock_type)
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
    ) => fizzle_fpurge(ctx) {
        let state = ctx.acquire();

        let file_id = FilePtr::from(stream);
        match state.local.file_objs.get(&file_id) {
            Some(_descriptor_id) => {
                unimplemented!("__fpurge()")
            }
            None => hook_macros::real!(__fpurge)(stream)
        }
    }
}
