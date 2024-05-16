use crate::{hook_macros, state};

hook_macros::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *const libc::c_void,
        count: libc::size_t
    ) -> libc::ssize_t => fizzle_read {

        crate::debug_abort("read");
        hook_macros::real!(read)(fd, buf, count)
    }
}

hook_macros::hook! {
    unsafe fn send(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_send {

        crate::debug_abort("send");
        hook_macros::real!(send)(fd, buf, len, flags)
    }
}

hook_macros::hook! {
    unsafe fn sendto(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t,
        flags: libc::c_int,
        dest_addr: *const libc::sockaddr,
        addrlen: libc::socklen_t
    ) -> libc::ssize_t => fizzle_sendto {

        crate::debug_abort("sendto");
        hook_macros::real!(sendto)(fd, buf, len, flags, dest_addr, addrlen)
    }
}

hook_macros::hook! {
    unsafe fn sendmsg(
        fd: libc::c_int,
        mst: *const libc::msghdr,
        flags: libc::c_int,
    ) -> libc::ssize_t => fizzle_sendmsg {
        hook_macros::real!(sendmsg)(fd, buf, len, flags, dest_addr, addrlen)
    }
}

hook_macros::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        count: libc::size_t
    ) -> libc::ssize_t => fizzle_write {
        hook_macros::real!(write)(fd, buf, count)
    }
}
