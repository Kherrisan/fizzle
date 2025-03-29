use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::{ptr, slice};

use crate::errno::Errno;
use crate::handlers::descriptor::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_write(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("write(fd={}, buf={:?}, len={}) -> ...", fd, buf, len);

        // Pass through AFL socket creation tasks
        #[cfg(feature = "afl")]
        if fd == 199 {
            return unsafe { libc::write(fd, buf, len) }
        }

        let s = slice::from_raw_parts(buf as *const u8, len);
        let mut iov = IoSlice::new(s);
        let write_data = WriteData::Iovec(slice::from_mut(&mut iov));

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(amount) => {
                crate::strace!("write(fd={}, buf={:?}, len={}) -> {}", fd, buf, len, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("write(fd={}, buf={:?}, len={}) -> -1 ({})", fd, buf, len, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn send(
        fd: libc::c_int,
        buf: *const libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_send(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("send(fd={}, buf={:?}, len={}, flags={}) -> ...", fd, buf, len, flags);

        let s = slice::from_raw_parts(buf as *const u8, len);
        let iov = IoSlice::new(s);

        let Some(write_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `send()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let mut buflen = 0;

        let mut data = SocketWriteData {
            buf: slice::from_ref(&iov),
            buflen: &mut buflen,
            addr_bytes: None,
            control_info: &[],
            msg_flags: SocketMsgFlags::empty(),
        };

        let write_data = WriteData::Socket(
            slice::from_mut(&mut data),
            write_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(msg_cnt) => {
                debug_assert!(msg_cnt <= 1);
                crate::strace!("send(fd={}, buf={:?}, len={}, flags={:?}) -> {}", fd, buf, len, write_flags, buflen);
                buflen as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("send(fd={}, buf={:?}, len={}, flags={:?}) -> -1 ({})", fd, buf, len, write_flags, e);
                e.set_errno();
                -1
            }
        }
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
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("sendto(fd={}, buf={:?}, len={}, flags={}, dest_addr={:?}, addrlen={}) -> ...", fd, buf, len, flags, dest_addr, addrlen);

        let s = slice::from_raw_parts(buf.cast::<u8>(), len);
        let iov = IoSlice::new(s);

        let Some(write_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `sendto()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let addr_bytes = if dest_addr.is_null() {
            None
        } else {
            Some(slice::from_raw_parts(dest_addr.cast::<u8>(), addrlen as usize))
        };

        let mut buflen = 0;

        let mut data = SocketWriteData {
            buf: slice::from_ref(&iov),
            buflen: &mut buflen,
            addr_bytes,
            control_info: &mut [],
            msg_flags: SocketMsgFlags::empty(),
        };

        let write_data = WriteData::Socket(
            slice::from_mut(&mut data),
            write_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(msg_cnt) => {
                debug_assert!(msg_cnt <= 1);
                crate::strace!("sendto(fd={}, buf={:?}, len={}, flags={:?}, src_addr={:?}, addrlen={:?}) -> {}", fd, buf, len, write_flags, dest_addr, addrlen, buflen);
                buflen as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("sendto(fd={}, buf={:?}, len={}, flags={:?}, src_addr={:?}, addrlen={:?}) -> -1 ({})", fd, buf, len, write_flags, dest_addr, addrlen, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sendmsg(
        fd: libc::c_int,
        msg: *const libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_sendmsg(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("sendmsg(fd={}, msg={:?}, flags={}) -> ...", fd, msg, flags);

        let iov_slice = slice::from_raw_parts((*msg).msg_iov.cast::<IoSlice<'_>>(), (*msg).msg_iovlen);

        let Some(write_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `recvmsg()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let addr_bytes = if (*msg).msg_name.is_null() {
            None
        } else {
            Some(slice::from_raw_parts((*msg).msg_name.cast::<u8>(), (*msg).msg_namelen as usize))
        };

        let Some(msg_flags) = SocketMsgFlags::from_bits((*msg).msg_flags) else {
            log::error!("unrecognized flags in `sendmsg()` msghdr: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let mut buflen = 0;

        let mut data = SocketWriteData {
            addr_bytes,
            buf: iov_slice,
            buflen: &mut buflen,
            msg_flags,
            control_info: &mut [],
        };

        let write_data = WriteData::Socket(
            slice::from_mut(&mut data),
            write_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(msg_cnt) => {
                debug_assert!(msg_cnt <= 1);
                crate::strace!("sendmsg(fd={}, msg={:?}, flags={}) -> {}", fd, msg, flags, buflen);
                buflen as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("sendmsg(fd={}, msg={:?}, flags={}) -> -1 ({})", fd, msg, flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn sendmmsg(
        fd: libc::c_int,
        msgvec: *mut libc::mmsghdr,
        vlen: libc::c_uint,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_sendmmsg(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("sendmmsg(fd={}, msgvec={:?}, vlen={}, flags={}) -> ...", fd, msgvec, vlen, flags);

        let msg_slice = slice::from_raw_parts_mut(msgvec, vlen as usize);

        let mut msgs = Vec::new();
        for msg in msg_slice {
            let iov_slice = slice::from_raw_parts((*msg).msg_hdr.msg_iov.cast::<IoSlice<'_>>(), (*msg).msg_hdr.msg_iovlen);
            let addr_bytes = if (*msg).msg_hdr.msg_name.is_null() {
                None
            } else {
                Some(slice::from_raw_parts((*msg).msg_hdr.msg_name.cast::<u8>(), (*msg).msg_hdr.msg_namelen as usize))
            };

            let control_info = if (*msg).msg_hdr.msg_control.is_null() {
                &[]
            } else {
                slice::from_raw_parts((*msg).msg_hdr.msg_control.cast::<u8>(), (*msg).msg_hdr.msg_controllen)
            };

            let Some(msg_flags) = SocketMsgFlags::from_bits((*msg).msg_hdr.msg_flags) else {
                log::error!("unrecognized flags in `sendmsg()` msghdr: {}", flags);
                Errno::EINVAL.set_errno();
                return -1
            };

            let buflen = &mut (*msg).msg_len;

            msgs.push(SocketWriteData {
                addr_bytes,
                buf: iov_slice,
                buflen,
                control_info,
                msg_flags,
            });
        }

        let Some(write_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `sendmmsg()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let write_data = WriteData::Socket(
            msgs.as_mut_slice(),
            write_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(amount) => {
                crate::strace!("sendmmsg(fd={}, msgvec={:?}, vlen={}, flags={}) -> {}", fd, msgvec, vlen, flags, amount);
                amount as i32
            }
            Err(e) => {
                crate::strace!("sendmmsg(fd={}, msgvec={:?}, vlen={}, flags={}) -> -1 ({})", fd, msgvec, vlen, flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t
    ) -> libc::ssize_t => fizzle_read(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("read(fd={}, buf={:?}, len={}) -> ...", fd, buf, len);

        let s = slice::from_raw_parts_mut(buf as *mut u8, len);
        let mut iov = IoSliceMut::new(s);
        let read_data = ReadData::Iovec(slice::from_mut(&mut iov));

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(amount) => {
                crate::strace!("read(fd={}, buf={:?}, len={}) -> {}", fd, buf, len, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("read(fd={}, buf={:?}, len={}) -> -1 ({})", fd, buf, len, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn recv(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        len: libc::size_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recv(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("recv(fd={}, buf={:?}, len={}, flags={}) -> ...", fd, buf, len, flags);

        let s = slice::from_raw_parts_mut(buf as *mut u8, len);
        let mut iov = IoSliceMut::new(s);

        let Some(read_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `recv()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let mut addrlen = 0;
        let mut msg_flags = SocketMsgFlags::empty();
        let mut buflen = 0;
        let mut control_len = 0;

        let mut data = SocketReadData {
            buf: slice::from_mut(&mut iov),
            buflen: &mut buflen,
            addr_bytes: &mut [],
            addrlen: &mut addrlen,
            control_info: &mut [],
            control_len: &mut control_len,
            msg_flags: &mut msg_flags,
        };

        let read_data = ReadData::Socket(
            slice::from_mut(&mut data),
            read_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(msg_cnt) => {
                debug_assert!(msg_cnt <= 1);
                crate::strace!("recv(fd={}, buf={:?}, len={}, flags={:?}) -> {}", fd, buf, len, read_flags, buflen);
                buflen as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("recv(fd={}, buf={:?}, len={}, flags={:?}) -> -1 ({})", fd, buf, len, read_flags, e);
                e.set_errno();
                -1
            }
        }
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
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("recvfrom(fd={}, buf={:?}, len={}, flags={}, src_addr={:?}, addrlen={:?}) -> ...", fd, buf, len, flags, src_addr, addrlen);

        let s = slice::from_raw_parts_mut(buf.cast::<u8>(), len);
        let mut iov = IoSliceMut::new(s);

        let Some(read_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `recvfrom()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let addr_bytes = if src_addr.is_null() || addrlen.is_null() {
            &mut []
        } else {
            slice::from_raw_parts_mut(src_addr.cast::<MaybeUninit<u8>>(), *addrlen as usize)
        };

        let mut msg_flags = SocketMsgFlags::empty();
        let mut buflen = 0;
        let mut control_len = 0;

        let mut data = SocketReadData {
            buf: slice::from_mut(&mut iov),
            buflen: &mut buflen,
            addr_bytes,
            addrlen: &mut *addrlen,
            control_info: &mut [],
            control_len: &mut control_len,
            msg_flags: &mut msg_flags,
        };

        let read_data = ReadData::Socket(
            slice::from_mut(&mut data),
            read_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(msg_cnt) => {
                debug_assert!(msg_cnt <= 1);
                crate::strace!("recvfrom(fd={}, buf={:?}, len={}, flags={:?}, src_addr={:?}, addrlen={:?}) -> {}", fd, buf, len, read_flags, src_addr, addrlen, buflen);
                buflen as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("recvfrom(fd={}, buf={:?}, len={}, flags={:?}, src_addr={:?}, addrlen={:?}) -> -1 ({})", fd, buf, len, read_flags, src_addr, addrlen, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn recvmsg(
        fd: libc::c_int,
        msg: *mut libc::msghdr,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_recvmsg(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("recvmsg(fd={}, msg={:?}, flags={}) -> ...", fd, msg, flags);

        let iov_slice = slice::from_raw_parts_mut((*msg).msg_iov.cast::<IoSliceMut<'_>>(), (*msg).msg_iovlen);

        let Some(read_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `recvmsg()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let addr_bytes = if (*msg).msg_name.is_null() {
            &mut []
        } else {
            slice::from_raw_parts_mut((*msg).msg_name.cast::<MaybeUninit<u8>>(), (*msg).msg_namelen as usize)
        };

        let msg_flags = &mut *(ptr::addr_of_mut!((*msg).msg_flags).cast::<SocketMsgFlags>());

        let addrlen = &mut (*msg).msg_namelen;
        let mut buflen = 0;
        let control_len = &mut (*msg).msg_controllen;

        let mut data = SocketReadData {
            addr_bytes,
            addrlen,
            buf: iov_slice,
            buflen: &mut buflen,
            msg_flags,
            control_info: &mut [],
            control_len,
        };

        let read_data = ReadData::Socket(
            slice::from_mut(&mut data),
            read_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(msg_cnt) => {
                debug_assert!(msg_cnt <= 1);
                crate::strace!("recvmsg(fd={}, msg={:?}, flags={}) -> {}", fd, msg, flags, buflen);
                buflen as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("recvmsg(fd={}, msg={:?}, flags={}) -> -1 ({})", fd, msg, flags, e);
                e.set_errno();
                -1
            }
        }
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
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> ...", fd, msgvec, vlen, flags, timeout);

        let msg_slice = slice::from_raw_parts_mut(msgvec, vlen as usize);

        let mut msgs = Vec::new();
        for msg in msg_slice {
            let iov_slice = slice::from_raw_parts_mut((*msg).msg_hdr.msg_iov.cast::<IoSliceMut<'_>>(), (*msg).msg_hdr.msg_iovlen);
            let addr_bytes = if (*msg).msg_hdr.msg_name.is_null() {
                &mut []
            } else {
                slice::from_raw_parts_mut((*msg).msg_hdr.msg_name.cast::<MaybeUninit<u8>>(), (*msg).msg_hdr.msg_namelen as usize)
            };
            let addrlen = &mut (*msg).msg_hdr.msg_namelen;

            let control_info = if (*msg).msg_hdr.msg_control.is_null() {
                &mut []
            } else {
                slice::from_raw_parts_mut((*msg).msg_hdr.msg_control.cast::<MaybeUninit<u8>>(), (*msg).msg_hdr.msg_controllen)
            };

            let control_len = &mut (*msg).msg_hdr.msg_controllen;

            let msg_flags = &mut *(ptr::addr_of_mut!((*msg).msg_hdr.msg_flags).cast::<SocketMsgFlags>());
            let buflen = &mut (*msg).msg_len;

            msgs.push(SocketReadData {
                addr_bytes,
                addrlen,
                buf: iov_slice,
                buflen,
                control_info,
                control_len,
                msg_flags,
            });
        }

        let Some(read_flags) = SocketFlags::from_bits(flags) else {
            log::error!("unrecognized flags in `recvmmsg()`: {}", flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let read_data = ReadData::Socket(
            msgs.as_mut_slice(),
            read_flags,
        );

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(amount) => {
                crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> {}", fd, msgvec, vlen, flags, timeout, amount);
                amount as i32
            }
            Err(e) => {
                crate::strace!("recvmmsg(fd={}, msgvec={:?}, vlen={}, flags={}, timeout={:?}) -> -1 ({})", fd, msgvec, vlen, flags, timeout, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn readv(
        fd: libc::c_int,
        iov: *mut libc::iovec, // NOTE: the definition has this as *const
        iovcnt: libc::c_int
    ) -> libc::ssize_t => fizzle_readv(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("readv(fd={}, iov={:?}, iovcnt={}) -> ...", fd, iov, iovcnt);

        let iov_slice = slice::from_raw_parts_mut(iov.cast::<IoSliceMut<'_>>(), iovcnt as usize);
        let read_data = ReadData::Iovec(iov_slice);

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(amount) => {
                crate::strace!("readv(fd={}, iov={:?}, iovcnt={}) -> {}", fd, iov, iovcnt, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("readv(fd={}, iov={:?}, iovcnt={}) -> -1 ({})", fd, iov, iovcnt, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn writev(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int
    ) -> libc::ssize_t => fizzle_writev(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("writev(fd={}, iov={:?}, iovcnt={}) -> ...", fd, iov, iovcnt);

        let iov_slice = slice::from_raw_parts(iov.cast::<IoSlice<'_>>(), iovcnt as usize);
        let write_data = WriteData::Iovec(iov_slice);

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(amount) => {
                crate::strace!("writev(fd={}, iov={:?}, iovcnt={}) -> {}", fd, iov, iovcnt, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("writev(fd={}, iov={:?}, iovcnt={}) -> -1 ({})", fd, iov, iovcnt, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pread(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pread(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("pread(fd={}, buf={:?}, count={}, offset={}) -> ...", fd, buf, count, offset);

        let s = slice::from_raw_parts_mut(buf as *mut u8, count);
        let mut iov = IoSliceMut::new(s);

        let read_data = ReadData::File(FileReadData {
            buf: slice::from_mut(&mut iov),
            offset: Some(offset),
            flags: FileFlags::empty(),
        });

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(amount) => {
                crate::strace!("pread(fd={}, buf={:?}, count={}, offset={}) -> {}", fd, buf, count, offset, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("pread(fd={}, buf={:?}, count={}, offset={}) -> {}", fd, buf, count, offset, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pwrite(
        fd: libc::c_int,
        buf: *const libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pwrite(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("pwrite(fd={}, buf={:?}, count={}, offset={}) -> ...", fd, buf, count, offset);

        let s = slice::from_raw_parts(buf.cast::<u8>(), count);
        let iov = IoSlice::new(s);

        let write_data = WriteData::File(FileWriteData {
            buf: slice::from_ref(&iov),
            offset: Some(offset),
            flags: FileFlags::empty(),
        });

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(amount) => {
                crate::strace!("pwrite(fd={}, buf={:?}, count={}, offset={}) -> {}", fd, buf, count, offset, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("pwrite(fd={}, buf={:?}, count={}, offset={}) -> {}", fd, buf, count, offset, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn preadv(
        fd: libc::c_int,
        iov: *mut libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_preadv(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("preadv(fd={}, iov={:?}, iovcnt={}, offset={}) -> ...", fd, iov, iovcnt, offset);

        let iov_slice = slice::from_raw_parts_mut(iov.cast::<IoSliceMut<'_>>(), iovcnt as usize);
        let read_data = ReadData::File(FileReadData {
            buf: iov_slice,
            offset: Some(offset),
            flags: FileFlags::empty(),
        });

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(amount) => {
                crate::strace!("preadv(fd={}, iov={:?}, iovcnt={}, offset={}) -> {}", fd, iov, iovcnt, offset, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("preadv(fd={}, iov={:?}, iovcnt={}, offset={}) -> {}", fd, iov, iovcnt, offset, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn pwritev(
        fd: libc::c_int,
        iov: *const libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t
    ) -> libc::ssize_t => fizzle_pwritev(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("pwritev(fd={}, iov={:?}, iovcnt={}, offset={}) -> ...", fd, iov, iovcnt, offset);

        let iov_slice = slice::from_raw_parts(iov.cast::<IoSlice<'_>>(), iovcnt as usize);
        let write_data = WriteData::File(FileWriteData {
            buf: iov_slice,
            offset: Some(offset),
            flags: FileFlags::empty(),
        });

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(amount) => {
                crate::strace!("pwritev(fd={}, iov={:?}, iovcnt={}, offset={}) -> {}", fd, iov, iovcnt, offset, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("pwritev(fd={}, iov={:?}, iovcnt={}, offset={}) -> {}", fd, iov, iovcnt, offset, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn preadv2(
        fd: libc::c_int,
        iov: *mut libc::iovec,
        iovcnt: libc::c_int,
        offset: libc::off_t,
        flags: libc::c_int
    ) -> libc::ssize_t => fizzle_preadv2(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("preadv2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={}) -> ...", fd, iov, iovcnt, offset, flags);

        let iov_slice = slice::from_raw_parts_mut(iov.cast::<IoSliceMut<'_>>(), iovcnt as usize);
        let Some(file_flags) = FileFlags::from_bits(flags) else {
            Errno::EINVAL.set_errno();
            return -1
        };

        let read_data = ReadData::File(FileReadData {
            buf: iov_slice,
            offset: Some(offset),
            flags: file_flags,
        });

        match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, read_data)) {
            Ok(amount) => {
                crate::strace!("preadv2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={:?}) -> {}", fd, iov, iovcnt, offset, file_flags, amount);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("preadv2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={:?}) -> {}", fd, iov, iovcnt, offset, file_flags, e);
                e.set_errno();
                -1
            }
        }
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
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("pwritev2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={}) -> ...", fd, iov, iovcnt, offset, flags);

        let iov_slice = slice::from_raw_parts(iov.cast::<IoSlice<'_>>(), iovcnt as usize);
        let Some(file_flags) = FileFlags::from_bits(flags) else {
            Errno::EINVAL.set_errno();
            return -1
        };

        let write_data = WriteData::File(FileWriteData {
            buf: iov_slice,
            offset: Some(offset),
            flags: file_flags,
        });

        match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, write_data)) {
            Ok(amount) => {
                crate::strace!("pwritev2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={:?}) -> {}", fd, iov, iovcnt, offset, amount, flags);
                amount as libc::ssize_t
            }
            Err(e) => {
                crate::strace!("pwritev2(fd={}, iov={:?}, iovcnt={}, offset={}, flags={:?}) -> {}", fd, iov, iovcnt, offset, e, flags);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn splice(
        _fd_in: libc::c_int,
        _off_in: *mut libc::off64_t,
        _fd_out: libc::c_int,
        _off_out: *mut libc::off64_t,
        _len: libc::size_t,
        _flags: libc::c_uint
    ) => fizzle_splice(_ctx) {
        unimplemented!("splice()")
    }
}

hook_macros::hook! {
    unsafe fn tee(
        _fd_in: libc::c_int,
        _fd_out: libc::c_int,
        _len: libc::size_t,
        _flags: libc::c_uint
    ) => fizzle_tee(_ctx) {
        unimplemented!("tee()")
    }
}

hook_macros::hook! {
    unsafe fn copy_file_range(
        _fd_in: libc::c_int,
        _off_in: *mut libc::off64_t,
        _fd_out: libc::c_int,
        _off_out: *mut libc::off64_t,
        _len: libc::size_t,
        _flags: libc::c_uint
    ) => fizzle_copy_file_range(_ctx) {
        unimplemented!("copy_file_range()")
    }
}
