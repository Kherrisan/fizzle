use std::ffi::{CStr, VaList};
use std::io::IoSlice;
use std::ptr;

use crate::errno::Errno;
use crate::handlers::filestream::*;
use crate::scheduler::Scheduler;

#[no_mangle]
pub unsafe extern "C" fn printf(format: *const libc::c_char, mut va_args: ...) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("printf() unimplemented for Fizzle internal use");
    };

    crate::strace!("printf(format={:?}, ...) -> ...", format);

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let res = crate::vasprintf(&raw mut out_string, format, va_args.as_va_list());
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!("printf(format={:?}, ...) -> -1 ({})", format_cstr, e);
        crate::hooks::post_hook();
        e.set_errno();
        return res;
    }

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    let stream_ptr = FilePtr::from_raw(unsafe { crate::stdout }).unwrap();

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

#[no_mangle]
pub unsafe extern "C" fn fprintf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: ...
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("fprintf() unimplemented for Fizzle internal use");
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

    let res = crate::vasprintf(&raw mut out_string, format, va_args.as_va_list());
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

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

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

#[no_mangle]
pub unsafe extern "C" fn dprintf(
    fd: libc::c_int,
    format: *const libc::c_char,
    mut va_args: ...
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("dprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("dprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vprintf(format: *const libc::c_char, mut va_args: VaList) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vprintf() unimplemented for Fizzle internal use");
    };

    crate::strace!("vprintf(format={:?}, ...) -> ...", format);

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let res = crate::vasprintf(&raw mut out_string, format, va_args.as_va_list());
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!("vprintf(format={:?}, ...) -> -1 ({})", format_cstr, e);
        crate::hooks::post_hook();
        e.set_errno();
        return res;
    }

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    let stream_ptr = FilePtr::from_raw(unsafe { crate::stdout }).unwrap();

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

#[no_mangle]
pub unsafe extern "C" fn vfprintf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vfprintf() unimplemented for Fizzle internal use");
    };

    crate::strace!(
        "vfprintf(stream={:?}, format={:?}, ...) -> ...",
        stream,
        format
    );

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

    let res = crate::vasprintf(&raw mut out_string, format, va_args.as_va_list());
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

#[no_mangle]
pub unsafe extern "C" fn vdprintf(
    fd: libc::c_int,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vdprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("vdprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn wprintf(format: *const libc::wchar_t, mut va_args: VaList) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("wprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("wprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn fwprintf(
    stream: *mut libc::FILE,
    format: *const libc::wchar_t,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("fwprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("fwprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vwprintf(
    format: *const libc::wchar_t,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vwprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("vwprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vfwprintf(
    stream: *mut libc::FILE,
    format: *const libc::wchar_t,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vfwprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("vfwprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn scanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("scanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("scanf(format={:?}) -> ...", format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc99_scanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_scanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc99_scanf(format={:?}) -> ...", format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc23_scanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_scanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc23_scanf(format={:?}) -> ...", format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn fscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("fscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("fscanf(stream={:?}, format={:?}) -> ...", stream, format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc99_fscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_fscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc99_fscanf(stream={:?}, format={:?}) -> ...", stream, format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc23_fscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_fscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc23_fscanf(stream={:?}, format={:?}) -> ...", stream, format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn vscanf(
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("vscanf(format={:?}, va_args=...) -> ...", format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc99_vscanf(
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_vscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc99_vscanf(format={:?}, va_args=...) -> ...", format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc23_vscanf(
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_vscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc99_vscanf(format={:?}, va_args=...) -> ...", format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn vfscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vfscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("vfscanf(stream={:?}, format={:?}, va_args=...) -> ...", stream, format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc99_vfscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_vfscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc99_vfscanf(stream={:?}, format={:?}, va_args=...) -> ...", stream, format);

    unimplemented!()
}

#[no_mangle]
pub unsafe extern "C" fn __isoc23_vfscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    mut va_args: VaList,
) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_vfscanf() unimplemented for Fizzle internal use");
    };

    crate::strace!("__isoc23_vfscanf(stream={:?}, format={:?}, va_args=...) -> ...", stream, format);

    unimplemented!()
}
