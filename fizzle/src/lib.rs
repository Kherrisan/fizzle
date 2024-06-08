#![feature(c_variadic)]

extern crate libc;

mod constants;
mod hook_macros;
mod hooks;
mod semaphore;
mod state;
mod streams;

pub(crate) use hook_macros::hook;

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::RawFd;
use std::{array, mem, ptr};

extern "C" {
    pub fn __afl_manual_init();
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
    if addr.is_null() {
        return Err(SockAddrError);
    }

    match (*addr).sa_family as i32 {
        libc::AF_INET => {
            let addr = addr as *const libc::sockaddr_in;
            if addrlen as usize != mem::size_of::<libc::sockaddr_in>() {
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
            if addrlen as usize != mem::size_of::<libc::sockaddr_in6>() {
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
unsafe fn encode_inet_address(addr: *mut libc::sockaddr, address: &SocketAddr) {
    match address {
        SocketAddr::V4(v4) => {
            let addr = addr as *mut libc::sockaddr_in;
            (*addr).sin_addr.s_addr = u32::from_be_bytes(v4.ip().octets()).to_be();
            (*addr).sin_port = v4.port().to_be();
        }
        SocketAddr::V6(v6) => {
            let addr = addr as *mut libc::sockaddr_in6;
            (*addr).sin6_addr.s6_addr = v6.ip().octets();
            (*addr).sin6_port = v6.port().to_be();
            (*addr).sin6_flowinfo = v6.flowinfo().to_be();
            (*addr).sin6_scope_id = v6.scope_id().to_be();
        }
    }
}
