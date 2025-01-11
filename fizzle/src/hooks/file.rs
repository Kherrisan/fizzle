use crate::handlers::file::FilePtr;
use crate::hook_macros;

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
    ) -> *mut libc::FILE => fizzle_fdopen(_ctx) {
        log::error!("fdopen() unimplemented");
        libc::fdopen(fd, mode)

    }
}

hook_macros::hook! {
    unsafe fn fopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fopen(_ctx) {
        log::error!("fopen unimplemented");
        libc::fopen(pathname, mode)
    }
}

hook_macros::hook! {
    unsafe fn freopen(
        pathname: *const libc::c_char,
        mode: *const libc::c_char,
        stream: *mut libc::FILE
    ) -> *mut libc::FILE => fizzle_freopen(_ctx) {
        log::error!("freopen() unimplemented");
        libc::freopen(pathname, mode, stream)
    }
}

hook_macros::hook! {
    unsafe fn fclose(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fclose(_ctx) {
        log::error!("fclose() unimplemented");
        libc::fclose(stream)
    }
}

hook_macros::hook! {
    unsafe fn fileno(stream: *mut libc::FILE) -> libc::c_int => fizzle_fileno(_ctx) {
        log::error!("fileno() unimplemented");
        libc::fileno(stream)
    }
}

hook_macros::hook! {
    unsafe fn fflush(stream: *mut libc::FILE) -> libc::c_int => fizzle_fflush(_ctx) {
        log::error!("fflush() unimplemented");
        libc::fflush(stream)
    }
}

hook_macros::hook! {
    unsafe fn fwrite(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwrite(_ctx) {
        log::error!("fwrite() unimplemented");
        libc::fwrite(ptr, size, nmemb, stream)
    }
}

hook_macros::hook! {
    unsafe fn fread(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fread(_ctx) {
        log::error!("fread() unimplemented");
        libc::fread(ptr, size, nmemb, stream)
    }
}

hook_macros::hook! {
    unsafe fn fgetc(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fgetc(_ctx) {
        log::error!("fgetc() unimplemented");
        libc::fgetc(stream)
    }
}

hook_macros::hook! {
    unsafe fn ungetc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ungetc(_ctx) {
        log::error!("ungetc() unimplemented");
        libc::ungetc(c, stream)
    }
}

hook_macros::hook! {
    unsafe fn fputc(
        c: libc::c_int,
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fputc(_ctx) {
        log::error!("fputc() unimplemented");
        libc::fputc(c, stream)
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
    ) -> libc::c_int => fizzle_fputs(_ctx) {
        log::error!("fputs() unimplemented");
        libc::fputs(s, stream)
    }
}

hook_macros::hook! {
    unsafe fn puts(
        s: *const libc::c_char
    ) -> libc::c_int => fizzle_puts(_ctx) {
        log::error!("puts() unimplemented");
        libc::puts(s)
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
