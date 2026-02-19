use std::ffi::{CStr, VaList};
use std::io::IoSlice;
use std::ptr;

use crate::errno::Errno;
use crate::external::{STDOUT, vasprintf};
use crate::handlers::filestream::*;
use crate::scheduler::Scheduler;
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn printf(format: *const libc::c_char, va_args: ...) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function printf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("printf() unimplemented for Fizzle internal use");
    };

    crate::strace!("printf(format={:?}, ...) -> ...", format);

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let res = vasprintf(&raw mut out_string, format, va_args);
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!("printf(format={:?}, ...) -> -1 ({})", format_cstr, e);
        crate::hooks::post_hook();
        e.set_errno();
        return res;
    }

    let out_cstr = CStr::from_ptr(out_string);
    let out_bytes = out_cstr.to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    crate::strace!(
        "fprintf wrote \"{:?}\"",
        out_cstr
    );

    let stream_ptr = FilePtr::from_raw(unsafe { STDOUT }).unwrap();

    match Scheduler::handle_event(
        &mut ctx,
        StreamWriteEvent::new(stream_ptr, &io_slice, 1, false),
    ) {
        Ok(()) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "printf(format={:?}, ...) -> {}",
                format_cstr,
                out_bytes.len()
            );
            crate::hooks::post_hook();
            out_bytes.len() as libc::c_int
        }
        Err(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!("printf(format={:?}, ...) -> {}", format_cstr, written);
            crate::hooks::post_hook();
            written as libc::c_int
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fprintf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: ...
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function fprintf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        let mut out_string = ptr::null_mut();
        let res = vasprintf(&raw mut out_string, format, va_args);
        if res < 0 {
            Errno::ENOMEM.set_errno();
            return libc::EOF
        }

        let ret = libc::fputs(out_string.cast_const(), stream);
        return ret
    };

    crate::strace!(
        "fprintf(stream={:?}, format={:?}, ...) -> ...",
        stream,
        format
    );

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let Some(stream_ptr) = FilePtr::from_raw(stream) else {
        crate::strace!(
            "fprintf(stream={:?}, format={:?}) -> -1 (EINVAL)",
            stream,
            format_cstr
        );
        Errno::EINVAL.set_errno();
        crate::hooks::post_hook();
        return libc::EOF;
    };

    let res = vasprintf(&raw mut out_string, format, va_args);
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!(
            "fprintf(stream={:?}, format={:?}, ...) -> -1 ({})",
            stream,
            format_cstr,
            e
        );
        e.set_errno();
        crate::hooks::post_hook();
        return res;
    }

    let out_cstr = CStr::from_ptr(out_string);
    let out_bytes = out_cstr.to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    crate::strace!(
        "fprintf wrote \"{:?}\"",
        out_cstr
    );

    match Scheduler::handle_event(
        &mut ctx,
        StreamWriteEvent::new(stream_ptr, &io_slice, 1, false),
    ) {
        Ok(()) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "fprintf(stream={:?}, format={:?}, ...) -> {}",
                stream,
                format_cstr,
                out_bytes.len()
            );
            crate::hooks::post_hook();
            out_bytes.len() as libc::c_int
        }
        Err(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "fprintf(stream={:?}, format={:?}, ...) -> {}",
                stream,
                format_cstr,
                written
            );
            crate::hooks::post_hook();
            written as libc::c_int
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __fprintf_chk(
    stream: *mut libc::FILE,
    flag: libc::c_int,
    format: *const libc::c_char,
    va_args: ...
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __fprintf_chk() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        let mut out_string = ptr::null_mut();
        let res = vasprintf(&raw mut out_string, format, va_args);
        if res < 0 {
            Errno::ENOMEM.set_errno();
            return libc::EOF
        }

        let ret = libc::fputs(out_string.cast_const(), stream);
        return ret
    };

    crate::strace!(
        "__fprintf_chk(stream={:?}, flag={}, format={:?}, ...) -> ...",
        stream,
        flag,
        format
    );

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let Some(stream_ptr) = FilePtr::from_raw(stream) else {
        crate::strace!(
            "__fprintf_chk(stream={:?}, flag={}, format={:?}) -> -1 (EINVAL)",
            stream,
            flag,
            format_cstr
        );
        Errno::EINVAL.set_errno();
        crate::hooks::post_hook();
        return libc::EOF;
    };

    let res = vasprintf(&raw mut out_string, format, va_args);
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!(
            "__fprintf_chk(stream={:?}, flag={}, format={:?}, ...) -> -1 ({})",
            stream,
            flag,
            format_cstr,
            e
        );
        e.set_errno();
        crate::hooks::post_hook();
        return res;
    }

    let out_cstr = CStr::from_ptr(out_string);
    let out_bytes = out_cstr.to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    crate::strace!(
        "__fprintf_chk wrote \"{:?}\"",
        out_cstr
    );

    match Scheduler::handle_event(
        &mut ctx,
        StreamWriteEvent::new(stream_ptr, &io_slice, 1, false),
    ) {
        Ok(()) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "__fprintf_chk(stream={:?}, flag={}, format={:?}, ...) -> {}",
                stream,
                flag,
                format_cstr,
                out_bytes.len()
            );
            crate::hooks::post_hook();
            out_bytes.len() as libc::c_int
        }
        Err(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "__fprintf_chk(stream={:?}, flag={}, format={:?}, ...) -> {}",
                stream,
                flag,
                format_cstr,
                written
            );
            crate::hooks::post_hook();
            written as libc::c_int
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dprintf(
    _fd: libc::c_int,
    _format: *const libc::c_char,
    _va_args: ...
) -> libc::c_int {
    let Some(_ctx) = crate::hooks::pre_hook() else {
        panic!("dprintf() unimplemented for Fizzle internal use");
    };

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function dprintf() called within signal handler")
        }
    }

    unimplemented!("dprintf()")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vprintf(format: *const libc::c_char, va_args: VaList) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vprintf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vprintf() unimplemented for Fizzle internal use");
    };

    crate::strace!("vprintf(format={:?}, ...) -> ...", format);

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let res = vasprintf(&raw mut out_string, format, va_args);
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!("vprintf(format={:?}, ...) -> -1 ({})", format_cstr, e);
        crate::hooks::post_hook();
        e.set_errno();
        return res;
    }

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    let stream_ptr = FilePtr::from_raw(unsafe { STDOUT }).unwrap();

    match Scheduler::handle_event(
        &mut ctx,
        StreamWriteEvent::new(stream_ptr, &io_slice, 1, false),
    ) {
        Ok(()) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "vprintf(format={:?}, ...) -> {}",
                format_cstr,
                out_bytes.len()
            );
            crate::hooks::post_hook();
            out_bytes.len() as libc::c_int
        }
        Err(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!("vprintf(format={:?}, ...) -> {}", format_cstr, written);
            crate::hooks::post_hook();
            written as libc::c_int
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vfprintf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: VaList,
) -> libc::c_int {   
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vfprintf() unimplemented for Fizzle internal use");
    };

    crate::strace!(
        "vfprintf(stream={:?}, format={:?}, ...) -> ...",
        stream,
        format
    );

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vfprintf() called within signal handler")
        }
    }

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let Some(stream_ptr) = FilePtr::from_raw(stream) else {
        crate::strace!(
            "vfprintf(stream={:?}, format={:?}) -> -1 (EINVAL)",
            stream,
            format_cstr
        );
        Errno::EINVAL.set_errno();
        crate::hooks::post_hook();
        return libc::EOF;
    };

    let res = vasprintf(&raw mut out_string, format, va_args);
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!(
            "vfprintf(stream={:?}, format={:?}, ...) -> -1 ({})",
            stream,
            format_cstr,
            e
        );
        e.set_errno();
        crate::hooks::post_hook();
        return res;
    }

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    match Scheduler::handle_event(
        &mut ctx,
        StreamWriteEvent::new(stream_ptr, &io_slice, 1, false),
    ) {
        Ok(()) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "vfprintf(stream={:?}, format={:?}, ...) -> {}",
                stream,
                format_cstr,
                out_bytes.len()
            );
            crate::hooks::post_hook();
            out_bytes.len() as libc::c_int
        }
        Err(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!(
                "vfprintf(stream={:?}, format={:?}, ...) -> {}",
                stream,
                format_cstr,
                written
            );
            crate::hooks::post_hook();
            written as libc::c_int
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vdprintf(
    _fd: libc::c_int,
    _format: *const libc::c_char,
    _va_args: VaList,
) -> libc::c_int {
    let Some(_ctx) = crate::hooks::pre_hook() else {
        panic!("vdprintf() unimplemented for Fizzle internal use");
    };

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vdprintf() called within signal handler")
        }
    }

    unimplemented!("vdprintf()")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wprintf(_format: *const libc::wchar_t, _va_args: VaList) -> libc::c_int {
    let Some(_ctx) = crate::hooks::pre_hook() else {
        panic!("wprintf() unimplemented for Fizzle internal use");
    };

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function wprintf() called within signal handler")
        }
    }

    unimplemented!("wprintf()")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fwprintf(
    _stream: *mut libc::FILE,
    _format: *const libc::wchar_t,
    _va_args: VaList,
) -> libc::c_int {
    let Some(_ctx) = crate::hooks::pre_hook() else {
        panic!("fwprintf() unimplemented for Fizzle internal use");
    };

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function fwprintf() called within signal handler")
        }
    }

    unimplemented!("fwprintf()")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vwprintf(
    _format: *const libc::wchar_t,
    _va_args: VaList,
) -> libc::c_int {
    let Some(_ctx) = crate::hooks::pre_hook() else {
        panic!("vwprintf() unimplemented for Fizzle internal use");
    };
    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vwprintf() called within signal handler")
        }
    }

    unimplemented!("vwprintf()")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vfwprintf(
    _stream: *mut libc::FILE,
    _format: *const libc::wchar_t,
    _va_args: VaList,
) -> libc::c_int {
    let Some(_ctx) = crate::hooks::pre_hook() else {
        panic!("vfwprintf() unimplemented for Fizzle internal use");
    };

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vfwprintf() called within signal handler")
        }
    }

    unimplemented!("vfwprintf()")
}
