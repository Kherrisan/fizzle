use std::mem::MaybeUninit;
use std::{cmp, ptr, slice};

use bitflags::bitflags;
use fizzle_common::io::{SockAddr, SockAddrError};

pub mod barrier;
pub mod buffer;
pub mod condvar;
pub mod descriptor;
pub mod directory;
pub mod entropy;
pub mod epoll;
pub mod eventfd;
pub mod file;
pub mod futex;
pub mod fuzz_endpoint;
pub mod id;
pub mod mq;
pub mod mutex;
pub mod pipe;
pub mod plugin;
pub mod polled;
pub mod poller;
pub mod process;
pub mod rwlock;
pub mod semaphore;
pub mod signal;
pub mod sleep;
pub mod socket;
pub mod spinlock;
pub mod thread;

bitflags! {
    #[derive(Debug)]
    pub struct MsgFlags: libc::c_int {
        const EOR = libc::MSG_EOR;
        const TRUNC = libc::MSG_TRUNC;
        const CTRUNC = libc::MSG_CTRUNC;
        const OOB = libc::MSG_OOB;
        const ERRQUEUE = libc::MSG_ERRQUEUE;
        const DONTWAIT = libc::MSG_DONTWAIT;
        const PEEK = libc::MSG_PEEK;
        const WAITALL = libc::MSG_WAITALL;
    }
}

pub trait MsgHdr: Sized {
    fn addr_bytes(&self) -> &[u8];

    fn addr(&self) -> Result<SockAddr, SockAddrError> {
        SockAddr::decode(self.addr_bytes())
    }

    fn vdata(&self) -> &[IoVec];

    fn ancillary_bytes(&self) -> &[u8];

    fn ancillary(&self) -> AncillaryReader<'_, Self>;

    fn flags(&self) -> &MsgFlags;
}

pub struct MsgHdrRef {
    pub msg_name: *mut libc::c_void,
    pub msg_namelen: libc::socklen_t,
    pub msg_iov: *mut IoVec,
    pub msg_iovlen: libc::size_t,
    pub msg_control: *mut libc::c_void,
    pub msg_controllen: libc::size_t,
    pub msg_flags: MsgFlags,
}

impl MsgHdr for MsgHdrRef {
    fn addr_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.msg_name as *const u8, self.msg_namelen as usize) }
    }

    fn vdata(&self) -> &[IoVec] {
        unsafe { slice::from_raw_parts(self.msg_iov as *const IoVec, self.msg_iovlen) }
    }

    fn ancillary_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.msg_control as *const u8, self.msg_controllen) }
    }

    fn ancillary(&self) -> AncillaryReader<'_, Self> {
        AncillaryReader {
            msghdr: self,
            curr_header: ptr::null(),
        }
    }

    fn flags(&self) -> &MsgFlags {
        &self.msg_flags
    }
}

#[repr(C)]
pub struct IoVec {
    pub iov_base: *mut libc::c_void,
    pub iov_len: libc::size_t,
}

impl IoVec {
    pub fn data(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.iov_base as *const u8, self.iov_len) }
    }

    pub fn data_mut(&self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.iov_base as *mut u8, self.iov_len) }
    }
}

pub struct AncillaryReader<'a, M: MsgHdr> {
    msghdr: &'a M,
    curr_header: *const libc::cmsghdr,
}

impl<M: MsgHdr> AncillaryReader<'_, M> {
    pub fn read(&mut self) -> Option<AncillaryData> {
        let header = if self.curr_header.is_null() {
            unsafe { libc::CMSG_FIRSTHDR(ptr::addr_of!(*self.msghdr) as *const libc::msghdr) }
        } else {
            unsafe {
                libc::CMSG_NXTHDR(
                    ptr::addr_of!(*self.msghdr) as *const libc::msghdr,
                    self.curr_header,
                )
            }
        };

        if header.is_null() {
            return None;
        }

        self.curr_header = header;

        unsafe {
            Some(AncillaryData {
                msg_level: (*header).cmsg_level,
                msg_type: (*header).cmsg_type,
                data: slice::from_raw_parts(
                    libc::CMSG_DATA(header) as *const u8,
                    (*header).cmsg_len,
                ),
            })
        }
    }
}

pub struct AncillaryData<'a> {
    pub msg_level: libc::c_int,
    pub msg_type: libc::c_int,
    pub data: &'a [u8],
}

pub struct MsgHdrOut {
    pub msg_name: *mut libc::c_void,
    pub msg_namelen: libc::socklen_t,
    pub msg_iov: *mut IoVecOut,
    pub msg_iovlen: libc::size_t,
    pub msg_control: *mut libc::c_void,
    pub msg_controllen: libc::size_t,
    pub msg_flags: MsgFlags,
}

impl MsgHdrOut {
    pub fn addr_bytes(&mut self) -> &mut [MaybeUninit<u8>] {
        if self.msg_name.is_null() {
            &mut []
        } else {
            unsafe {
                slice::from_raw_parts_mut(
                    self.msg_name as *mut MaybeUninit<u8>,
                    self.msg_namelen as usize,
                )
            }
        }
    }

    pub fn set_addrlen(&mut self, len: u32) {
        assert!(self.msg_namelen >= len);
        self.msg_namelen = len;
    }

    pub fn vdata_mut(&mut self) -> &mut [IoVecOut] {
        unsafe { slice::from_raw_parts_mut(self.msg_iov, self.msg_iovlen) }
    }

    pub fn ancillary_bytes(&mut self) -> &mut [MaybeUninit<u8>] {
        if self.msg_control.is_null() {
            &mut []
        } else {
            unsafe {
                slice::from_raw_parts_mut(
                    self.msg_control as *mut MaybeUninit<u8>,
                    self.msg_controllen,
                )
            }
        }
    }

    pub fn set_ancillary_len(&mut self, len: usize) {
        assert!(self.msg_controllen >= len);
        self.msg_controllen = len;
    }

    pub fn ancillary(&mut self) -> AncillaryWriter<'_> {
        AncillaryWriter {
            msghdr: self,
            curr_header: ptr::null(),
        }
    }

    pub fn flags_mut(&mut self) -> &mut MsgFlags {
        &mut self.msg_flags
    }
}

#[repr(C)]
pub struct IoVecOut {
    pub iov_base: *mut libc::c_void,
    pub iov_len: libc::size_t,
}

impl IoVecOut {
    pub fn data_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe { slice::from_raw_parts_mut(self.iov_base as *mut MaybeUninit<u8>, self.iov_len) }
    }
}

pub struct AncillaryWriter<'a> {
    msghdr: &'a mut MsgHdrOut,
    curr_header: *const libc::cmsghdr,
}

impl AncillaryWriter<'_> {
    pub fn write(&mut self, ancillary: AncillaryData<'_>) -> Result<(), AncillaryError> {
        if self.msghdr.msg_control.is_null() {
            return Err(AncillaryError::Truncated);
        }

        let header = if self.curr_header.is_null() {
            unsafe { libc::CMSG_FIRSTHDR(ptr::addr_of_mut!(*self.msghdr) as *const libc::msghdr) }
        } else {
            unsafe {
                libc::CMSG_NXTHDR(
                    ptr::addr_of_mut!(*self.msghdr) as *const libc::msghdr,
                    self.curr_header,
                )
            }
        };

        if header.is_null() {
            return Err(AncillaryError::Truncated);
        }

        unsafe {
            (*header).cmsg_level = ancillary.msg_level;
            (*header).cmsg_type = ancillary.msg_type;
            (*header).cmsg_len = libc::CMSG_LEN(ancillary.data.len() as u32) as usize;
            let cmsg_data = libc::CMSG_DATA(header);
            let remaining_len = self.msghdr.msg_controllen.saturating_sub(
                cmsg_data.offset_from(self.msghdr.msg_control as *const u8) as usize,
            );

            let trunc_len = cmp::min(ancillary.data.len(), remaining_len);

            ptr::copy_nonoverlapping(ancillary.data.as_ptr(), cmsg_data, trunc_len);

            if remaining_len < libc::CMSG_SPACE((*header).cmsg_len as u32) as usize {
                Err(AncillaryError::Truncated)
            } else {
                self.curr_header = header;
                Ok(())
            }
        }
    }
}

impl Drop for AncillaryWriter<'_> {
    fn drop(&mut self) {
        if !self.curr_header.is_null() {
            unsafe {
                let len = self
                    .curr_header
                    .byte_offset_from(ptr::addr_of!(*self.msghdr.msg_control))
                    as usize
                    + libc::CMSG_SPACE((*self.curr_header).cmsg_len as u32) as usize;
                self.msghdr.msg_controllen = len;
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AncillaryError {
    /// The provided message exceeded the available ancillary space and was truncated.
    Truncated,
    //    /// The given data source did not have an ancillary message available within it.
    //    InsufficientData,
}
