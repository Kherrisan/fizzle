#![feature(c_variadic)]
#![feature(new_uninit)]

extern crate libc;

mod constants;
mod hook_macros;
pub mod hooks;
mod semaphore;
mod state;
mod streams;

use fizzle_common::io::{UnixAddr, MAX_UNIX_ABSTRACT_LEN, MAX_UNIX_PATH_LEN};
use fizzle_common::path::FilePath;
use fizzle_common::storage::Buffer;
pub(crate) use hook_macros::hook;

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::RawFd;
use std::{array, cmp, mem, ptr, slice};

extern "C" {
    #[cfg(feature = "afl")]
    pub fn __afl_manual_init();

    // TODO: three underscores for Apple
    #[cfg(feature = "pcr")]
    pub fn __afl_persistent_loop(input: libc::c_uint) -> libc::c_int;

    #[cfg(feature = "pcr")]
    pub static __afl_fuzz_len: *mut libc::c_uint;

    #[cfg(feature = "pcr")]
    pub static __afl_fuzz_ptr: *mut libc::c_uchar;

    #[cfg(feature = "pcr")]
    pub static __afl_connected: libc::c_int;

    #[cfg(feature = "pcr")]
    pub static mut __afl_sharedmem_fuzzing: libc::c_int;
}

pub fn report_strict_failure(explanation: &'static str) {
    debug_assert!(false, "{}", explanation);
    log::error!("{}", explanation);
}

/// Creates a new location in memory that is guaranteed to be unique to others.
/// This is particularly useful in emulating hooks that require a pointer as a return value.
/// Memory locations should be destroyed with `unique_mem_destroy()` once finished using.
unsafe fn unique_mem_create() -> *mut libc::c_void {
    // TODO: turn this into an alias creator that uses sequential addresses in allocated to handle these opaque references more efficiently.

    let addr = libc::mmap(
        ptr::null_mut(),
        1,
        libc::PROT_NONE,
        libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
        -1,
        0,
    );
    if addr.is_null() {
        panic!("failed to create unique memory handle via `mmap`");
    }

    addr
}

/// Unmaps a location in memory created with `unique_mem_create()`.
/// This uses `munmap` under the hood; it is unsafe to call this on any `mem_location` other than those returned by `unique_mem_create()`.
unsafe fn unique_mem_destroy(mem_location: *mut libc::c_void) {
    let res = unsafe { libc::munmap(mem_location, 1) };
    if res != 0 {
        panic!("error during destruction of unique memory handle via `mmap`");
    }
}

fn alias_fd_create() -> RawFd {
    let fd = unsafe { libc::memfd_create(c"FIZZLE_ALIAS_FD".as_ptr(), 0) };
    if fd < 0 {
        panic!("fizzle internal file descriptor alias creation (`memfd_create`) failed");
    }
    fd
}

fn alias_fd_destroy(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}

pub struct SockAddrError;

unsafe fn decode_inet_address(
    addr: *const libc::sockaddr,
    addrlen: libc::socklen_t,
) -> Result<SocketAddr, SockAddrError> {
    if addr.is_null() || addrlen < 2 {
        return Err(SockAddrError);
    }

    match (*addr).sa_family as i32 {
        libc::AF_INET => {
            let addr = addr as *const libc::sockaddr_in;
            if (addrlen as usize) < mem::size_of::<libc::sockaddr_in>() {
                return Err(SockAddrError);
            }

            // TODO: verify correctness of these conversions
            let addr_bytes = u32::from_be((*addr).sin_addr.s_addr).to_be_bytes();
            let port = u16::from_be((*addr).sin_port);
            Ok(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(addr_bytes[0], addr_bytes[1], addr_bytes[2], addr_bytes[3]),
                port,
            )))
        }
        libc::AF_INET6 => {
            let addr = addr as *const libc::sockaddr_in6;
            if (addrlen as usize) < mem::size_of::<libc::sockaddr_in6>() {
                return Err(SockAddrError);
            }

            // TODO: verify correctness of these conversions
            let addr_segments: [u16; 8] = array::from_fn(|i| {
                u16::from_be_bytes(
                    (*addr).sin6_addr.s6_addr[2 * i..(2 * i) + 2]
                        .try_into()
                        .unwrap(),
                )
            }); // TODO: replace with newer libc functions when they arrive
            let port = u16::from_be((*addr).sin6_port);
            let flow_info = u32::from_be((*addr).sin6_flowinfo);
            let scope_id = u32::from_be((*addr).sin6_scope_id);
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::new(
                    addr_segments[0],
                    addr_segments[1],
                    addr_segments[2],
                    addr_segments[3],
                    addr_segments[4],
                    addr_segments[5],
                    addr_segments[6],
                    addr_segments[7],
                ),
                port,
                flow_info,
                scope_id,
            )))
        }
        _ => panic!(
            "fizzle does not currently support address family {}",
            (*addr).sa_family
        ),
    }
}

///
/// # Safety
///
/// It is the responsibility of the caller to ensure that `addr` points to valid bytes that are
/// sized according to the address family in the address (e.g., the address length for an `AF_INET`
/// sockaddr should be equal to `mem::size_of::<libc::sockaddr_in>()`).
fn encode_inet_address(addr: *mut libc::sockaddr, addrlen: *mut libc::socklen_t, address: &SocketAddr) {
    let mut storage = MaybeUninit::<libc::sockaddr_storage>::uninit();

    unsafe {
        match address {
            SocketAddr::V4(v4) => {
                let storage_addr = ptr::addr_of_mut!(storage) as *mut libc::sockaddr_in;
                (*storage_addr).sin_family = libc::AF_INET as u16;
                (*storage_addr).sin_addr.s_addr = u32::from_be_bytes(v4.ip().octets()).to_be();
                (*storage_addr).sin_port = v4.port().to_be();

                *addrlen = cmp::min(*addrlen, mem::size_of::<libc::sockaddr_in>() as u32);
                ptr::copy_nonoverlapping(storage_addr, addr as *mut libc::sockaddr_in, *addrlen as usize);
            }
            SocketAddr::V6(v6) => {
                let storage_addr = ptr::addr_of_mut!(storage) as *mut libc::sockaddr_in6;
                (*storage_addr).sin6_family = libc::AF_INET6 as u16;
                (*storage_addr).sin6_addr.s6_addr = v6.ip().octets();
                (*storage_addr).sin6_port = v6.port().to_be();
                (*storage_addr).sin6_flowinfo = v6.flowinfo().to_be();
                (*storage_addr).sin6_scope_id = v6.scope_id().to_be();

                *addrlen = cmp::min(*addrlen, mem::size_of::<libc::sockaddr_in6>() as u32);
                ptr::copy_nonoverlapping(storage_addr, addr as *mut libc::sockaddr_in6, *addrlen as usize);
            }
        }
    }
}

fn decode_unix_address(
    addr: *const libc::sockaddr,
    addrlen: libc::socklen_t,
) -> Result<UnixAddr, SockAddrError> {
    unsafe {
        if addr.is_null() || addrlen < 2 || (*addr).sa_family != libc::AF_UNIX as u16 {
            return Err(SockAddrError);
        }

        let unix_addr = addr as *const libc::sockaddr_un;
        let unix_path = ptr::addr_of!((*unix_addr).sun_path) as *const u8;
        if addrlen == 2 {
            Ok(UnixAddr::Unnamed)
        } else if *unix_path == 0 {
            let abstract_path = slice::from_raw_parts(unix_path.add(1), cmp::min(MAX_UNIX_ABSTRACT_LEN, addrlen as usize - 3));

            let mut abstract_buf = Buffer::new();
            abstract_buf.write(abstract_path);

            Ok(UnixAddr::Abstract(abstract_buf))
        } else {
            let Ok(path) = CStr::from_bytes_until_nul(slice::from_raw_parts(unix_path, cmp::min(MAX_UNIX_PATH_LEN, addrlen as usize - 2))) else {
                return Err(SockAddrError) // Unix path must be null-terminated
            };

            let Ok(file) = FilePath::from_cstr(path) else {
                return Err(SockAddrError)
            };

            Ok(UnixAddr::Pathname(file))
        }
    }
}

///
/// # Safety
///
/// It is the responsibility of the caller to ensure that `addr` points to valid bytes that are
/// sized according to the address family in the address (e.g., the address length for an `AF_INET`
/// sockaddr should be equal to `mem::size_of::<libc::sockaddr_in>()`).
fn encode_unix_address(addr: *mut libc::sockaddr, addrlen: *mut libc::socklen_t, address: &UnixAddr) {
    let mut storage = MaybeUninit::<libc::sockaddr_storage>::uninit();
    let storage_addr = ptr::addr_of_mut!(storage) as *mut libc::sockaddr_un;

    unsafe {
        (*storage_addr).sun_family = libc::AF_UNIX as u16;
        
        *addrlen = match address {
            UnixAddr::Abstract(abstract_addr) => {
                let path_ptr = ptr::addr_of_mut!((*storage_addr).sun_path) as *mut u8;
                ptr::copy_nonoverlapping(abstract_addr.data().as_ptr(), path_ptr, abstract_addr.data().len());
                cmp::min(*addrlen, 2 + abstract_addr.data().len() as u32)
            }
            UnixAddr::Pathname(pathname_addr) => {
                let path_ptr = ptr::addr_of_mut!((*storage_addr).sun_path) as *mut u8;
                ptr::copy_nonoverlapping(pathname_addr.data().as_ptr(), path_ptr, pathname_addr.data().len());
                cmp::min(*addrlen, 2 + pathname_addr.data().len() as u32)
            }
            UnixAddr::Unnamed => cmp::min(*addrlen, 2),
        };

        ptr::copy_nonoverlapping(storage_addr, addr as *mut libc::sockaddr_un, *addrlen as usize);
    }
}
