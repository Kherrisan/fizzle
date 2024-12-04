use crate::hook_macros;

#[allow(non_camel_case_types)]
type io_uring_params = libc::c_void;

hook_macros::hook! {
    unsafe fn io_uring_setup(
        _entries: u32,
        _p: *mut io_uring_params
    ) -> libc::ssize_t => fizzle_io_uring_setup(_ctx) {
        unimplemented!("io_uring_setup")
    }
}

hook_macros::hook! {
    unsafe fn io_uring_register(
        _fd: libc::c_int,
        _opcode: libc::c_uint,
        _arg: *mut libc::c_void,
        _nr_args: libc::c_uint
    ) -> libc::ssize_t => fizzle_io_uring_register(_ctx) {
        unimplemented!("io_uring_register")
    }
}

hook_macros::hook! {
    unsafe fn io_uring_enter(
        _fd: libc::c_uint,
        _to_submit: libc::c_uint,
        _min_complete: libc::c_uint,
        _flags: libc::c_uint,
        _sig: *mut libc::sigset_t
    ) -> libc::ssize_t => fizzle_io_uring_enter(_ctx) {
        unimplemented!("io_uring_enter")
    }
}

hook_macros::hook! {
    unsafe fn io_uring_enter2(
        _fd: libc::c_uint,
        _to_submit: libc::c_uint,
        _min_complete: libc::c_uint,
        _flags: libc::c_uint,
        _sig: *mut libc::sigset_t,
        _sz: libc::size_t
    ) -> libc::c_int => fizzle_io_uring_enter2(_ctx) {
        unimplemented!("io_uring_enter2")
    }
}
