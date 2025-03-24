use crate::hook_macros;

hook_macros::hook! {
    unsafe fn getaddrinfo(
        node: *const libc::c_char,
        service: *const libc::c_char,
        hints: *const libc::addrinfo,
        res: *mut *mut libc::addrinfo
    ) -> libc::c_int => fizzle_getaddrinfo(ctx) {
        unimplemented!("getaddrinfo")
    }
}

hook_macros::hook! {
    unsafe fn gethostbyaddr(
        addr: *const libc::c_void,
        len: libc::socklen_t,
        ty: libc::c_int
    ) -> *mut libc::hostent => fizzle_gethostbyaddr(ctx) {
        unimplemented!("gethostbyaddr")
    }
}

hook_macros::hook! {
    unsafe fn gethostbyname(
        name: *const char
    ) -> *mut libc::hostent => fizzle_gethostbyname(ctx) {
        unimplemented!("gethostbyname")
    }
}

hook_macros::hook! {
    unsafe fn getnameinfo(
        addr: *mut libc::sockaddr,
        addrlen: libc::socklen_t,
        host: *mut libc::c_char,
        hostlen: libc::socklen_t,
        serv: *mut libc::c_char,
        servlen: *mut libc::socklen_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_getnameinfo(ctx) {
        unimplemented!("getnameinfo")
    }
}