use std::ffi::{CStr, CString};
use std::mem::{self, MaybeUninit};
use std::{ptr, slice};

use fizzle_common::io::SockAddr;

use crate::handlers::netdb::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

// TODO: Upstream this.
const EAI_ADDRFAMILY: libc::c_int = -9;

hook_macros::hook! {
    unsafe fn getaddrinfo(
        node: *const libc::c_char,
        service: *const libc::c_char,
        hints: *const libc::addrinfo,
        res: *mut *mut libc::addrinfo
    ) -> libc::c_int => fizzle_getaddrinfo(ctx) {
        crate::strace!("getaddrinfo(node={:?}, service={:?}, hints={:?}, res={:?}) -> ...", node, service, hints, res);

        let node_cstr = if node.is_null() {
            None
        } else {
            Some(CStr::from_ptr(node))
        };

        let service_cstr = if service.is_null() {
            None
        } else {
            Some(CStr::from_ptr(service))
        };

        if res.is_null() {
            panic!("invalid null `res` passed to getaddrinfo")
        }

        let hint_family = if hints.is_null() {
            libc::AF_UNSPEC
        } else {
            (*hints).ai_family
        };

        let hint_socktype = if hints.is_null() {
            0
        } else {
            (*hints).ai_socktype
        };


        let hint_protocol = if hints.is_null() {
            0
        } else {
            (*hints).ai_protocol
        };

        let hint_flags = if hints.is_null() {
            GetAddrInfoFlags::empty()
        } else {
            GetAddrInfoFlags::from_bits((*hints).ai_flags).unwrap() // TODO: more flags for FileZilla
        };

        match Scheduler::handle_event(&mut ctx, GetAddressInfoEvent::new(node_cstr, service_cstr, hint_family, hint_socktype, hint_protocol, hint_flags)) {
            Ok(addr_info) => {
                if hints.is_null() {
                    crate::strace!("getaddrinfo(node={:?}, service={:?}, hints=None, res={:?}) -> 0", node_cstr, service_cstr, res);
                } else {
                    crate::strace!("getaddrinfo(node={:?}, service={:?}, hints={{family={}, socktype={}, protocol={}, flags={:?}}}, res={:?}) -> 0", node_cstr, service_cstr, hint_family, hint_socktype, hint_protocol, hint_flags, res);
                }
                *res = Box::into_raw(addr_info);
                0
            },
            Err((errno, ret)) => {
                if hints.is_null() {
                    crate::strace!("getaddrinfo(node={:?}, service={:?}, hints=None, res={:?}) -> {} ({})", node_cstr, service_cstr, res, ret, errno);
                } else {
                    crate::strace!("getaddrinfo(node={:?}, service={:?}, hints={{family={}, socktype={}, protocol={}, flags={:?}}}, res={:?}) -> {} ({})", node_cstr, service_cstr, hint_family, hint_socktype, hint_protocol, hint_flags, res, ret, errno);
                }
                errno.set_errno();
                ret
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn freeaddrinfo(
        res: *mut libc::addrinfo
    ) => fizzle_freeaddrinfo(ctx) {
        crate::strace!("freeaddrinfo(res={:?}) -> ...", res);

        match Scheduler::handle_event(&mut ctx, FreeAddressInfoEvent::new(res)) {
            Ok(()) => {
                crate::strace!("freeaddrinfo(res={:?}) -> ()", res);
            },
            Err(_) => unreachable!(),
        }
    }
}

#[repr(C)]
pub struct gaicb {
    ar_name: *const libc::c_char,
    ar_service: *const libc::c_char,
    ar_request: *const libc::addrinfo,
    ar_result: *mut libc::addrinfo,
}

hook_macros::hook! {
    unsafe fn getaddrinfo_a(
        mode: libc::c_int,
        list: *mut gaicb,
        nitems: libc::c_int,
        sevp: *mut libc::sigevent
    ) -> libc::c_int => fizzle_getaddrinfo_a(_ctx) {
        unimplemented!("getaddrinfo_a()")
    }
}

hook_macros::hook! {
    unsafe fn gai_suspend(
        list: *const gaicb,
        nitems: libc::c_int,
        timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_gai_suspend(_ctx) {
        unimplemented!("gai_suspend()")
    }
}

hook_macros::hook! {
    unsafe fn gai_error(
        req: *mut gaicb
    ) -> libc::c_int => fizzle_gai_error(_ctx) {
        unimplemented!("gai_error()")
    }
}

hook_macros::hook! {
    unsafe fn gai_cancel(
        req: *mut gaicb
    ) -> libc::c_int => fizzle_gai_cancel(_ctx) {
        unimplemented!("gai_cancel()")
    }
}

hook_macros::hook! {
    unsafe fn gethostbyaddr(
        addr: *const libc::c_void,
        len: libc::socklen_t,
        ty: libc::c_int
    ) -> *mut libc::hostent => fizzle_gethostbyaddr(ctx) {
        // deprecated--should we implement??
        unimplemented!("gethostbyaddr")
    }
}

hook_macros::hook! {
    unsafe fn gethostbyname(
        name: *const libc::c_char
    ) -> *mut libc::hostent => fizzle_gethostbyname(ctx) {
        crate::strace!("gethostbyname(name={:?}) -> ...", name);

        let name_cstr = CStr::from_ptr(name);

        let addr_info = match Scheduler::handle_event(&mut ctx, GetAddressInfoEvent::new(Some(name_cstr), None, libc::AF_INET, 0, 0, GetAddrInfoFlags::empty())) {
            Ok(addr_info) => addr_info,
            Err((errno, _ret)) => {
                errno.set_errno();
                crate::strace!("gethostbyname(name={:?}) -> NULL", name);
                return ptr::null_mut()
            }
        };

        let mut hostent: Box<libc::hostent> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });

        let aliases = Box::new([ptr::null_mut::<libc::c_char>()]);
        let addr_list = Box::new([addr_info.ai_addr, ptr::null_mut()]);

        hostent.h_name = CString::into_raw(name_cstr.to_owned());
        hostent.h_addrtype = addr_info.ai_family;
        hostent.h_addr_list = Box::into_raw(addr_list).cast();
        hostent.h_aliases = Box::into_raw(aliases).cast();
        hostent.h_length = addr_info.ai_addrlen as i32;

        // TODO: we just leak everything here...
        let ptr = Box::into_raw(hostent);

        crate::strace!("gethostbyname(name={:?}) -> {:?}", name, ptr);
        ptr
    }
}

hook_macros::hook! {
    unsafe fn gethostbyname2(
        name: *const libc::c_char,
        af: libc::c_int
    ) -> *mut libc::hostent => fizzle_gethostbyname2(ctx) {
        crate::strace!("gethostbyname2(name={:?}, af={}) -> ...", name, af);

        let name_cstr = CStr::from_ptr(name);

        let addr_info = match Scheduler::handle_event(&mut ctx, GetAddressInfoEvent::new(Some(name_cstr), None, af, 0, 0, GetAddrInfoFlags::empty())) {
            Ok(addr_info) => addr_info,
            Err((errno, _ret)) => {
                errno.set_errno();
                crate::strace!("gethostbyname2(name={:?}, af={}) -> NULL", name, af);
                return ptr::null_mut()
            }
        };

        let mut hostent: Box<libc::hostent> = Box::new(unsafe { MaybeUninit::zeroed().assume_init() });

        let aliases = Box::new([ptr::null_mut::<libc::c_char>()]);
        let addr_list = Box::new([addr_info.ai_addr, ptr::null_mut()]);

        hostent.h_name = CString::into_raw(name_cstr.to_owned());
        hostent.h_addrtype = addr_info.ai_family;
        hostent.h_addr_list = Box::into_raw(addr_list).cast();
        hostent.h_aliases = Box::into_raw(aliases).cast();
        hostent.h_length = addr_info.ai_addrlen as i32;

        // TODO: we just leak everything here...
        let ptr = Box::into_raw(hostent);

        crate::strace!("gethostbyname2(name={:?}, af={}) -> {:?}", name, af, ptr);
        ptr
    }
}

hook_macros::hook! {
    unsafe fn getnameinfo(
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t,
        host: *mut libc::c_char,
        hostlen: libc::socklen_t,
        serv: *mut libc::c_char,
        servlen: libc::socklen_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_getnameinfo(ctx) {
        crate::strace!("getnameinfo(addr={:?}, addrlen={}, host={:?}, hostlen={}, serv={:?}, servlen={}, flags={}) -> ...", addr, addrlen, host, hostlen, serv, servlen, flags);

        let Ok(sockaddr) = SockAddr::decode(unsafe {
            slice::from_raw_parts(addr.cast(), addrlen as usize)
        }) else {
            log::warn!("getnameinfo() received invalid address");
            crate::strace!("getnameinfo(addr={:?}, addrlen={}, host={:?}, hostlen={}, serv={:?}, servlen={}, flags={}) -> EAI_FAMILY", addr, addrlen, host, hostlen, serv, servlen, flags);
            return libc::EAI_FAMILY
        };

        let host_bytes = if host.is_null() {
            None
        } else {
            Some(unsafe {
                slice::from_raw_parts_mut(host.cast(), hostlen as usize)
            })
        };

        let serv_bytes = if serv.is_null() {
            None
        } else {
            Some(unsafe {
                slice::from_raw_parts_mut(serv.cast(), servlen as usize)
            })
        };

        let Some(flags) = GetNameInfoFlags::from_bits(flags) else {
            log::warn!("unrecognized flags in getnameinfo()");
            crate::strace!("getnameinfo(addr={:?}, host={:?}, serv={:?}, flags={}) -> EAI_BADFLAGS", sockaddr, host_bytes, serv_bytes, flags);
            return libc::EAI_BADFLAGS
        };

        let ev_res = Scheduler::handle_event(&mut ctx, GetNameInfoEvent::new(sockaddr.clone(), host_bytes, serv_bytes, flags));

        let host_out = if host.is_null() {
            None
        } else if ev_res.is_err() {
            Some(c"")
        } else {
            Some(unsafe {
                CStr::from_bytes_until_nul(slice::from_raw_parts_mut(host.cast::<u8>(), hostlen as usize)).unwrap()
            })
        };

        let serv_out = if serv.is_null() {
            None
        } else if ev_res.is_err() {
            Some(c"")
        } else {
            Some(unsafe {
                CStr::from_bytes_until_nul(slice::from_raw_parts_mut(serv.cast::<u8>(), servlen as usize)).unwrap()
            })
        };

        match ev_res {
            Ok(()) => {
                crate::strace!("getnameinfo(addr={:?}, host={:?}, serv={:?}, flags={:?}) -> 0", sockaddr, host_out, serv_out, flags);
                0
            },
            Err(ret) => {
                crate::strace!("getnameinfo(addr={:?}, host={:?}, serv={:?}, flags={:?}) -> 0", sockaddr, host_out, serv_out, flags);
                ret
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn getservbyname(
        name: *const libc::c_char,
        proto: *const libc::c_char
    ) -> *mut libc::servent => fizzle_getservbyname(ctx) {
        let name_cstr = CStr::from_ptr(name);
        let proto_cstr = if proto.is_null() {
            None
        } else {
            Some(CStr::from_ptr(proto))
        };

        crate::strace!("getservbyname(name={:?}, proto={:?}) -> ...", name_cstr, proto_cstr);

        let proto_hint = match proto_cstr {
            None => 0,
            Some(s) if s == c"tcp" => libc::IPPROTO_TCP,
            Some(s) if s == c"udp" => libc::IPPROTO_UDP,
            Some(s) => {
                log::warn!("unrecognized service {:?} passed to getservbyname()", s);

                crate::strace!("getservbyname(name={:?}, proto={:?}) -> NULL", name_cstr, proto_cstr);
                return ptr::null_mut()
            }
        };

        let addr_info = match Scheduler::handle_event(&mut ctx, GetAddressInfoEvent::new(None, Some(name_cstr), libc::AF_INET, 0, proto_hint, GetAddrInfoFlags::empty())) {
            Ok(addr_info) => addr_info,
            Err((errno, _ret)) => {
                errno.set_errno();
                crate::strace!("getservbyname(name={:?}, proto={:?}) -> NULL", name_cstr, proto_cstr);
                return ptr::null_mut()
            }
        };

        let aliases = Box::new(ptr::null_mut());
        let port = if addr_info.ai_addrlen == mem::size_of::<libc::sockaddr_in>() as u32 {
            (*addr_info.ai_addr.cast::<libc::sockaddr_in>()).sin_port
        } else if addr_info.ai_addrlen == mem::size_of::<libc::sockaddr_in6>() as u32 {
            (*addr_info.ai_addr.cast::<libc::sockaddr_in6>()).sin6_port
        } else {
            unreachable!()
        };

        let proto = match addr_info.ai_protocol {
            libc::IPPROTO_TCP => c"tcp",
            libc::IPPROTO_UDP => c"udp",
            _ => unreachable!(),
        };

        let servent: Box<libc::servent> = Box::new(libc::servent {
            s_name: CString::into_raw(CString::from(name_cstr)),
            s_aliases: Box::into_raw(aliases),
            s_port: port as i32,
            s_proto: proto.as_ptr().cast_mut(),
        });

        match Scheduler::handle_event(&mut ctx, FreeAddressInfoEvent::new(Box::into_raw(addr_info))) {
            Ok(()) => (),
            Err(()) => unreachable!(),
        }

        let ptr = Box::into_raw(servent);
        crate::strace!("getservbyname(name={:?}, proto={:?}) -> {:?}", name_cstr, proto_cstr, ptr);

        // TODO: this is clearly a memory leak,
        return ptr
    }
}
