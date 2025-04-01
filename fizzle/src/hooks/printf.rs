use std::ffi::CStr;
use std::io::IoSlice;
use std::ptr;

use crate::errno::Errno;
use crate::handlers::filestream::*;
use crate::scheduler::Scheduler;

/*

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
        return res
    }

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    let stream_ptr = FilePtr::from_raw(unsafe { crate::stdout }).unwrap();

    match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, 1)) {
        Ok(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!("printf(format={:?}, ...) -> {}", format_cstr, written);
            // TODO: need to handle (or fail on) non-blocking I/O case here?
            assert!(written == out_bytes.len());
            crate::hooks::post_hook();
            written as libc::c_int
        }
        Err(e) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!("printf(format={:?}, ...) -> -1 ({})", format_cstr, e);
            e.set_errno();
            crate::hooks::post_hook();
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn fprintf(stream: *mut libc::FILE, format: *const libc::c_char, mut va_args: ...) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("fprintf() unimplemented for Fizzle internal use");
    };

    crate::strace!("fprintf(stream={:?}, format={:?}, ...) -> ...", stream, format);

    let format_cstr = CStr::from_ptr(format);
    let mut out_string = ptr::null_mut();

    let Some(stream_ptr) = FilePtr::from_raw(stream) else {
        crate::strace!("fprintf(stream={:?}, format={:?}) -> -1 (EINVAL)", stream, format_cstr);
        Errno::EINVAL.set_errno();
        crate::hooks::post_hook();
        return libc::EOF
    };

    let res = crate::vasprintf(&raw mut out_string, format, va_args.as_va_list());
    if res < 0 {
        let e = Errno::get_errno();
        crate::strace!("fprintf(stream={:?}, format={:?}, ...) -> -1 ({})", stream, format_cstr, e);
        e.set_errno();
        crate::hooks::post_hook();
        return res
    }

    let out_bytes = CStr::from_ptr(out_string).to_bytes();
    let io_slice = IoSlice::new(out_bytes);

    match Scheduler::handle_event(&mut ctx, FileStreamWriteEvent::new(stream_ptr, &io_slice, 1)) {
        Ok(written) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!("fprintf(stream={:?}, format={:?}, ...) -> 0", stream, format_cstr);
            // TODO: need to handle (or fail on) non-blocking I/O case here?
            assert!(written == out_bytes.len());
            crate::hooks::post_hook();
            written as libc::c_int
        }
        Err(e) => {
            libc::free(out_string.cast::<libc::c_void>());
            crate::strace!("fprintf(stream={:?}, format={:?}, ...) -> -1 ({})", stream, format_cstr, e);
            e.set_errno();
            crate::hooks::post_hook();
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn dprintf(fd: libc::c_int, format: *const libc::c_char, mut va_args: ...) -> libc::c_int {
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("dprintf() unimplemented for Fizzle internal use");
    };

    unimplemented!("dprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vprintf(format: *const libc::c_char, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("vprintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("vprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vfprintf(stream: *mut libc::FILE, format: *const libc::c_char, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("vfprintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("vfprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vdprintf(fd: libc::c_int, format: *const libc::c_char, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("vdrintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("vdprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn wprintf(format: *const libc::wchar_t, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("wprintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("wprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn fwprintf(stream: *mut libc::FILE, format: *const libc::wchar_t, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("fwprintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("fwprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vwprintf(format: *const libc::wchar_t, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("vwprintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("vwprintf()")
}

#[no_mangle]
pub unsafe extern "C" fn vfwprintf(stream: *mut libc::FILE, format: *const libc::wchar_t, mut va_args: VaList) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("vfwprintf() unimplemented for Fizzle internal use")
    }
    crate::state::set_entered_handler(true);

    // SAFETY: only one FizzleSingleton is ever owned at a time
    let mut ctx = fizzle_singleton(); // TODO: should fizzle_singleton() just be a thread-local variable? Would that improve safety?

    unimplemented!("vfwprintf()")
}

*/
