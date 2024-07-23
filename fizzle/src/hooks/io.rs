use std::ptr;

use crate::handlers::descriptor::{DescriptorId, DescriptorInfo};
use crate::handlers::{FfiOutput, IoVec, IoVecOut, MsgFlags, MsgHdrOut, MsgHdrRef};
use crate::hook_macros;

const PIPE_BUF: usize = 4096;
const IOV_MAX: usize = 16;

hook_macros::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_write(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::write(fd, buf, len);
            crate::strace!("write(fd={}, buf={:?}, len={}) -> {} (errno {})", fd, buf, len, res, *libc::__errno_location());
            return res
        };

        let mut iovec = IoVec {
            iov_base: buf as *mut libc::c_void,
            iov_len: len,
        };

        let mut msg = MsgHdrRef {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: ptr::addr_of_mut!(iovec),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::empty(),
        };

        drop(state);

        crate::strace!("write(fd={}, buf={:?}, len={}) -> ...", fd, buf, len);
        let res = descriptor_id.write(&mut ctx, &mut msg);
        crate::strace!("write(fd={}, buf={:?}, len={}) -> {}", fd, buf, len, res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn send(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_send(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::send(fd, buf, len, flags);
            crate::strace!("send(fd={}, buf={:?}, len={}, flags={}) -> {} (errno {})", fd, buf, len, flags, res, *libc::__errno_location());
            return res
        };

        let mut iovec = IoVec {
            iov_base: buf as *mut libc::c_void,
            iov_len: len,
        };

        let mut msg = MsgHdrRef {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: ptr::addr_of_mut!(iovec),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::from_bits(flags).unwrap(),
        };

        drop(state);

        crate::strace!("send(fd={}, buf={:?}, len={}, flags={}) -> ...", fd, buf, len, flags);
        let res = descriptor_id.write(&mut ctx, &mut msg);
        crate::strace!("send(fd={}, buf={:?}, len={}, flags={}) -> {}", fd, buf, len, flags, res.display());

        res.out()
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
    ) -> libc::ssize_t => fizzle_sendto(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::sendto(fd, buf, len, flags, dest_addr, addrlen);
            crate::strace!("sendto(fd={}, buf={:?}, len={}, flags={}, dest_addr={:?}, addrlen={}) -> {} (errno {})", fd, buf, len, flags, dest_addr, addrlen, res, *libc::__errno_location());
            return res
        };

        let mut iovec = IoVec {
            iov_base: buf as *mut libc::c_void,
            iov_len: len,
        };

        let mut msg = MsgHdrRef {
            msg_name: dest_addr as *mut libc::c_void,
            msg_namelen: addrlen,
            msg_iov: ptr::addr_of_mut!(iovec),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::from_bits(flags).unwrap(),
        };

        drop(state);

        crate::strace!("sendto(fd={}, buf={:?}, len={}, flags={}, dest_addr={:?}, addrlen={}) -> ...", fd, buf, len, flags, dest_addr, addrlen);
        let res = descriptor_id.write(&mut ctx, &mut msg);
        crate::strace!("sendto(fd={}, buf={:?}, len={}, flags={}, dest_addr={:?}, addrlen={}) -> {}", fd, buf, len, flags, dest_addr, addrlen, res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn sendmsg(
        fd: libc::c_int,
        msg: *const libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_sendmsg(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::sendmsg(fd, msg, flags);
            crate::strace!("sendmsg(fd={}, msg={:?}, flags={}) -> {} (errno {})", fd, msg, flags, res, *libc::__errno_location());
            return res
        };

        let msg_mut = &mut *(msg as *mut MsgHdrRef);
        msg_mut.msg_flags = MsgFlags::from_bits(flags).unwrap();

        drop(state);

        crate::strace!("sendmsg(fd={}, msg={:?}, flags={}) -> ...", fd, msg, flags);
        let res = descriptor_id.write(&mut ctx, msg_mut);
        crate::strace!("sendmsg(fd={}, msg={:?}, flags={}) -> {}", fd, msg, flags, res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn sendmmsg(
        fd: libc::c_int,
        msgvec: *mut libc::mmsghdr,
        vlen: libc::c_uint,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_sendmmsg(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::sendmmsg(fd, msgvec, vlen, flags);
            crate::strace!("sendmmsg(fd={}, msgvec={:?}, vlen={}, flags={}) -> {} (errno {})", fd, msgvec, vlen, flags, res, *libc::__errno_location());
            return res
        };

        if vlen == 0 {
            return 0
        }

        let msg = &mut *(ptr::addr_of_mut!((*msgvec).msg_hdr) as *mut MsgHdrRef);
        msg.msg_flags = MsgFlags::from_bits(flags).unwrap();
        let msg_len = &mut *(ptr::addr_of_mut!((*msgvec).msg_len));

        drop(state);

        crate::strace!("sendmmsg(fd={}, msgvec={:?}, vlen={}, flags={}) -> ...", fd, msgvec, vlen, flags);
        let res = descriptor_id.write(&mut ctx, msg);
        crate::strace!("sendmmsg(fd={}, msgvec={:?}, vlen={}, flags={}) -> {}", fd, msgvec, vlen, flags, res.display());

        if res.out() >= 0 {
            *msg_len = res.out() as u32;
            1
        } else {
            -1
        }
    }
}

hook_macros::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_read(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::read(fd, buf, len);
            crate::strace!("recv(fd={}, buf={:?}, len={}) -> {} (errno {})", fd, buf, len, res, *libc::__errno_location());
            return res
        };

        let mut iovec = IoVecOut {
            iov_base: buf,
            iov_len: len,
        };

        let mut msg = MsgHdrOut {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: ptr::addr_of_mut!(iovec),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::empty(),
        };

        drop(state);

        crate::strace!("read(fd={}, buf={:?}, len={}) -> ...", fd, buf, len);
        let res = descriptor_id.read(&mut ctx, &mut msg);
        crate::strace!("read(fd={}, buf={:?}, len={}) -> {}", fd, buf, len, res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn recv(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recv(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::recv(fd, buf, len, flags);
            crate::strace!("recv(fd={}, buf={:?}, len={}, flags={}) -> {} (errno {})", fd, buf, len, flags, res, *libc::__errno_location());
            return res
        };

        let mut iovec = IoVecOut {
            iov_base: buf,
            iov_len: len,
        };

        let mut msg = MsgHdrOut {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: ptr::addr_of_mut!(iovec),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::from_bits(flags).unwrap(),
        };

        drop(state);

        crate::strace!("recv(fd={}, buf={:?}, len={}, flags={:?}) -> ...", fd, buf, len, msg.flags_mut());
        let res = descriptor_id.read(&mut ctx, &mut msg);
        crate::strace!("recv(fd={}, buf={:?}, len={}, flags={:?}) -> {}", fd, buf, len, msg.flags_mut(), res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn recvfrom(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t,
        flags: libc::c_int,
        src_addr: *mut libc::sockaddr,
        addrlen: *mut libc::socklen_t
    ) -> libc::ssize_t => fizzle_recvfrom(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::recvfrom(fd, buf, len, flags, src_addr, addrlen);
            crate::strace!("recvfrom(fd={}, buf={:?}, len={}, flags={}, src_addr={:?}, addrlen={:?}) -> {} (errno {})", fd, buf, len, flags, src_addr, addrlen, res, *libc::__errno_location());
            return res
        };

        let mut iovec = IoVecOut {
            iov_base: buf,
            iov_len: len,
        };

        let mut msg = MsgHdrOut {
            msg_name: src_addr as *mut libc::c_void,
            msg_namelen: if addrlen.is_null() { 0 } else { *addrlen },
            msg_iov: ptr::addr_of_mut!(iovec),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::from_bits(flags).unwrap()
        };

        drop(state);

        crate::strace!("recvfrom(fd={}, buf={:?}, len={}, flags={:?}, addr={:?}) -> ...", fd, buf, len, msg.flags_mut(), src_addr);
        let res = descriptor_id.read(&mut ctx, &mut msg);
        crate::strace!("recvfrom(fd={}, buf={:?}, len={}, flags={:?}, addr={:?}) -> {}", fd, buf, len, msg.flags_mut(), src_addr, res.display());

        if res.is_ok() && !addrlen.is_null()  {
            *addrlen = msg.msg_namelen;
        }

        res.out()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_camel_case_types)]
struct sctp_shutdown_event {
    spc_type: u16,
    spc_flags: u16,
    spc_length: u32,
    sse_assoc_id: libc::sctp_assoc_t,
}

const SCTP_SHUTDOWN_EVENT: u16 = (1 << 15) + 5;

hook_macros::hook! {
    unsafe fn recvmsg(
        fd: libc::c_int,
        msg: *mut libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recvmsg(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::recvmsg(fd, msg, flags);
            crate::strace!("recvmsg(fd={}, msg={:?}, flags={}) -> {} (errno {})", fd, msg, flags, res, *libc::__errno_location());
            return res
        };

        let msg_out = &mut *(msg as *mut MsgHdrOut);
        *msg_out.flags_mut() = MsgFlags::from_bits(flags).unwrap();

        drop(state);

        crate::strace!("recvmsg(fd={}, msg={:?}, flags={:?}) -> ...", fd, msg, msg_out.flags_mut());
        let res = descriptor_id.read(&mut ctx, msg_out);
        crate::strace!("recvmsg(fd={}, msg={:?}, flags={:?}) -> {}", fd, msg, msg_out.flags_mut(), res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn recvmmsg(
        fd: libc::c_int,
        msgvec: *mut libc::mmsghdr,
        vlen: libc::c_uint,
        flags: libc::c_int,
        timeout: *mut libc::timespec
    ) -> libc::c_int => fizzle_recvmmsg(ctx) {
        let state = ctx.acquire();

        // TODO: account for timeout
        // NOTE: recvmmsg currently only receives one message at a time

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::recvmmsg(fd, msgvec, vlen, flags, timeout);
            crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> {} (errno {})", fd, msgvec, vlen, flags, timeout, res, *libc::__errno_location());
            return res
        };

        if vlen <= 0 {
            crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> 0 (empty msgvec)", fd, msgvec, vlen, flags, timeout);
            return 0
        }

        let msg = &mut *(ptr::addr_of_mut!((*msgvec).msg_hdr) as *mut MsgHdrOut);
        msg.msg_flags = MsgFlags::from_bits(flags).unwrap();
        let msg_len = &mut *(ptr::addr_of_mut!((*msgvec).msg_len));

        drop(state);

        crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> ...", fd, msgvec, vlen, flags, timeout);
        let res = descriptor_id.read(&mut ctx, msg);
        crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> {}", fd, msgvec, vlen, flags, timeout, res.display());

        if res.out() >= 0 {
            *msg_len = res.out() as u32;
            1
        } else {
            -1
        }
    }
}

hook_macros::hook! {
    unsafe fn readv(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int
    ) -> libc::ssize_t => fizzle_readv(_ctx) {

        crate::report_strict_failure("`readv` unimplemented");
        hook_macros::real!(readv)(fd, iov, iovcnt)
    }
}

hook_macros::hook! {
    unsafe fn writev(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int
    ) -> libc::ssize_t => fizzle_writev(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::writev(fd, iov, iovcnt);
            crate::strace!("writev(fd={}, iov={:?}, iovcnt={}) -> {} (errno {})", fd, iov, iovcnt, res, *libc::__errno_location());
            return res
        };

        let msg = MsgHdrRef {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: iov as *mut IoVec,
            msg_iovlen: iovcnt as usize,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: MsgFlags::empty()
        };

        drop(state);

        crate::strace!("writev(fd={}, iov={:?}, iovcnt={}) -> ...", fd, iov, iovcnt);
        let res = descriptor_id.write(&mut ctx, &msg);
        crate::strace!("writev(fd={}, iov={:?}, iovcnt={}) -> {}", fd, iov, iovcnt, res.display());

        res.out()
    }
}

hook_macros::hook! {
    unsafe fn pread(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pread(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::pread(fd, buf, count, offset);
            crate::strace!("pread(fd={}, buf={:?}, cnt={}, offset={}) -> {} (errno {})", fd, buf, count, offset, res, *libc::__errno_location());
            return res
        };

        panic!("`pread()` unimplemented by Fizzle")
    }
}

hook_macros::hook! {
    unsafe fn pwrite(
        fd: libc::c_int,
        buf: *const libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pwrite(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::pwrite(fd, buf, count, offset);
            crate::strace!("pwrite(fd={}, buf={:?}, count={}, offset={}) -> {} (errno {})", fd, buf, count, offset, res, *libc::__errno_location());
            return res
        };

        panic!("`pwritev()` unimplemented by Fizzle")
    }
}

hook_macros::hook! {
    unsafe fn preadv(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_preadv(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::preadv(fd, iov, iovcnt, offset);
            crate::strace!("preadv(fd={}, iov={:?}, iovcnt={}, offset={}) -> {} (errno {})", fd, iov, iovcnt, offset, res, *libc::__errno_location());
            return res
        };

        panic!("`preadv()` unimplemented by Fizzle")
    }
}

hook_macros::hook! {
    unsafe fn pwritev(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pwritev(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::pwritev(fd, iov, iovcnt, offset);
            crate::strace!("pwritev(fd={}, iov={:?}, iovcnt={}, offset={}) -> {} (errno {})", fd, iov, iovcnt, offset, res, *libc::__errno_location());
            return res
        };

        panic!("`pwritev()` unimplemented by Fizzle")
    }
}

hook_macros::hook! {
    unsafe fn preadv2(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_preadv2(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::preadv2(fd, iov, iovcnt, offset, flags);
            crate::strace!("preadv2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={}) -> {} (errno {})", fd, iov, iovcnt, offset, flags, res, *libc::__errno_location());
            return res
        };

        panic!("`preadv2()` unimplemented by Fizzle")
    }
}

hook_macros::hook! {
    unsafe fn pwritev2(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_pwritev2(ctx) {
        let state = ctx.acquire();

        let descriptor_id = DescriptorId::from_raw_fd(fd);

        if let Some(DescriptorInfo { is_passthrough: true, .. }) = state.local.fds.get(&descriptor_id) {
            let res = libc::pwritev(fd, iov, iovcnt, offset);
            crate::strace!("pwritev2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={}) -> {} (errno {})", fd, iov, iovcnt, offset, flags, res, *libc::__errno_location());
            return res
        };

        panic!("`pwritev2()` unimplemented by Fizzle")
    }
}
