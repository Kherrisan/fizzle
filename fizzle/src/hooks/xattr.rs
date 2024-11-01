use crate::hook_macros;

hook_macros::hook! {
    unsafe fn getxattr(
        _path: *const libc::c_char,
        _name: *const libc::c_char,
        _value: *mut libc::c_void,
        _size: libc::size_t
    ) => fizzle_clearerr(_ctx) {
        unimplemented!("getxattr()")
    }
}

hook_macros::hook! {
    unsafe fn lgetxattr(
        _path: *const libc::c_char,
        _name: *const libc::c_char,
        _value: *mut libc::c_void,
        _size: libc::size_t
    ) => fizzle_lgetxattr(_ctx) {
        unimplemented!("lgetxattr()")
    }
}

hook_macros::hook! {
    unsafe fn fgetxattr(
        _fd: libc::c_int,
        _name: *const libc::c_char,
        _value: *mut libc::c_void,
        _size: libc::size_t
    ) => fizzle_fgetxattr(_ctx) {
        unimplemented!("fgetxattr()")
    }
}

hook_macros::hook! {
    unsafe fn setxattr(
        _path: *const libc::c_char,
        _name: *const libc::c_char,
        _value: *const libc::c_void,
        _size: libc::size_t
    ) => fizzle_setxattr(_ctx) {
        unimplemented!("setxattr()")
    }
}

hook_macros::hook! {
    unsafe fn lsetxattr(
        _path: *const libc::c_char,
        _name: *const libc::c_char,
        _value: *const libc::c_void,
        _size: libc::size_t
    ) => fizzle_lsetxattr(_ctx) {
        unimplemented!("lsetxattr()")
    }
}

hook_macros::hook! {
    unsafe fn fsetxattr(
        _fd: libc::c_int,
        _name: *const libc::c_char,
        _value: *const libc::c_void,
        _size: libc::size_t
    ) => fizzle_fsetxattr(_ctx) {
        unimplemented!("fsetxattr()")
    }
}

hook_macros::hook! {
    unsafe fn listxattr(
        _path: *const libc::c_char,
        _list: *const libc::c_char,
        _size: libc::size_t
    ) => fizzle_listxattr(_ctx) {
        unimplemented!("listxattr()")
    }
}

hook_macros::hook! {
    unsafe fn llistxattr(
        _path: *const libc::c_char,
        _list: *const libc::c_char,
        _size: libc::size_t
    ) => fizzle_llistxattr(_ctx) {
        unimplemented!("llistxattr()")
    }
}

hook_macros::hook! {
    unsafe fn flistxattr(
        _fd: libc::c_int,
        _list: *const libc::c_char,
        _size: libc::size_t
    ) => fizzle_flistxattr(_ctx) {
        unimplemented!("flistxattr()")
    }
}

hook_macros::hook! {
    unsafe fn removexattr(
        _path: *const libc::c_char,
        _name: *const libc::c_char
    ) => fizzle_removexattr(_ctx) {
        unimplemented!("removexattr()")
    }
}

hook_macros::hook! {
    unsafe fn lremovexattr(
        _path: *const libc::c_char,
        _name: *const libc::c_char
    ) => fizzle_lremovexattr(_ctx) {
        unimplemented!("lremovexattr()")
    }
}

hook_macros::hook! {
    unsafe fn fremovexattr(
        _fd: libc::c_int,
        _name: *const libc::c_char
    ) => fizzle_fremovexattr(_ctx) {
        unimplemented!("fremovexattr()")
    }
}
