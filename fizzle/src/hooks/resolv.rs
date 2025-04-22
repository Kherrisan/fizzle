use std::ffi::CStr;
use std::{cmp, ptr, slice};

use crate::hook_macros;
use crate::scheduler::Scheduler;
use crate::handlers::resolv::*;

hook_macros::hook! {
    unsafe fn res_query(
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_query(ctx) {
        let mut buf = [0u8; 1024];

        crate::strace!("res_query(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> ...", dname, class, ty, answer, anslen);

        let len = crate::res_mkquery(0, dname, class, ty, ptr::null_mut(), 0, ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if len < 0 {
            log::error!("res_mkquery() failed for res_query");
            crate::strace!("res_query(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", dname, class, ty, answer, anslen);
            return -1
        }

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(&buf[..len as usize])) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_query(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> {}", dname, class, ty, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_query(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", dname, class, ty, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_nquery(
        statep: *mut libc::c_void,
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_nquery(ctx) {
        let mut buf = [0u8; 1024];

        let dname_cstr = CStr::from_ptr(dname);

        crate::strace!("res_nquery(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> ...", statep, dname_cstr, class, ty, answer, anslen);

        let len = crate::res_mkquery(0, dname, class, ty, ptr::null_mut(), 0, ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if len < 0 {
            log::error!("res_mkquery() failed for res_nquery with dname {:?}", dname_cstr);
            crate::strace!("res_nquery(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", statep, dname_cstr, class, ty, answer, anslen);
            return -1
        }

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(&buf[..len as usize])) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_nquery(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> {}", statep, dname_cstr, class, ty, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_nquery(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", statep, dname_cstr, class, ty, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_querydomain(
        name: *const libc::c_char,
        domain: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_querydomain(ctx) {
        let mut buf = [0u8; 1024];

        let name_cstr = CStr::from_ptr(name);
        let domain_cstr = CStr::from_ptr(domain);

        crate::strace!("res_querydomain(name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> ...", name_cstr, domain_cstr, class, ty, answer, anslen);

        let mut dname_vec = Vec::new();
        dname_vec.extend_from_slice(name_cstr.to_bytes());
        dname_vec.push(b'.');
        dname_vec.extend_from_slice(domain_cstr.to_bytes());
        dname_vec.push(b'\0');
        let dname = dname_vec.as_ptr().cast();

        let len = crate::res_mkquery(0, dname, class, ty, ptr::null_mut(), 0, ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if len < 0 {
            log::error!("res_mkquery() failed for res_querydomain");
            crate::strace!("res_querydomain(name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", name_cstr, domain_cstr, class, ty, answer, anslen);
            return -1
        }

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(&buf[..len as usize])) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_querydomain(name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> {}", name, domain, class, ty, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_querydomain(name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", name, domain, class, ty, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_nquerydomain(
        statep: *mut libc::c_void,
        name: *const libc::c_char,
        domain: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_nquerydomain(ctx) {
        let mut buf = [0u8; 1024];

        let name_cstr = CStr::from_ptr(name);
        let domain_cstr = CStr::from_ptr(domain);

        crate::strace!("res_nquerydomain(statep={:?}, name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> ...", statep, name_cstr, domain_cstr, class, ty, answer, anslen);

        let mut dname_vec = Vec::new();
        dname_vec.extend_from_slice(name_cstr.to_bytes());
        dname_vec.push(b'.');
        dname_vec.extend_from_slice(domain_cstr.to_bytes());
        dname_vec.push(b'\0');
        let dname = dname_vec.as_ptr().cast();

        let len = crate::res_mkquery(0, dname, class, ty, ptr::null_mut(), 0, ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if len < 0 {
            log::error!("res_mkquery() failed for res_nquerydomain");
            crate::strace!("res_nquerydomain(statep={:?}, name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", statep, name_cstr, domain_cstr, class, ty, answer, anslen);
            return -1
        }

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(&buf[..len as usize])) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_nquerydomain(statep={:?}, name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> {}", statep, name, domain, class, ty, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_nquerydomain(statep={:?}, name={:?}, domain={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", statep, name, domain, class, ty, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_search(
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_search(ctx) {
        let mut buf = [0u8; 1024];

        crate::strace!("res_search(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> ...", dname, class, ty, answer, anslen);

        let len = crate::res_mkquery(0, dname, class, ty, ptr::null_mut(), 0, ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if len < 0 {
            log::error!("res_mkquery() failed for res_query");
            crate::strace!("res_search(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", dname, class, ty, answer, anslen);
            return -1
        }

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(&buf[..len as usize])) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_search(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> {}", dname, class, ty, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_search(dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", dname, class, ty, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_nsearch(
        statep: *mut libc::c_void,
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_nsearch(ctx) {
        let mut buf = [0u8; 1024];

        crate::strace!("res_nsearch(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> ...", statep, dname, class, ty, answer, anslen);

        let len = crate::res_mkquery(0, dname, class, ty, ptr::null_mut(), 0, ptr::null_mut(), buf.as_mut_ptr(), 1024);
        if len < 0 {
            log::error!("res_mkquery() failed for res_nsearch");
            crate::strace!("res_nsearch(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", statep, dname, class, ty, answer, anslen);
            return -1
        }

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(&buf[..len as usize])) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_nsearch(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> {}", statep, dname, class, ty, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_nsearch(statep={:?}, dname={:?}, class={}, ty={}, answer={:?}, anslen={}) -> -1", statep, dname, class, ty, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_send(
        msg: *const libc::c_char,
        msglen: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_send(ctx) {
        crate::strace!("res_send(msg={:?}, msglen={}, answer={:?}, anslen={}) -> ...", msg, msglen, answer, anslen);

        let msg_slice = slice::from_raw_parts(msg.cast::<u8>(), msglen as usize);

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(msg_slice)) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_send(msg={:?}, msglen={}, answer={:?}, anslen={}) -> {}", msg, msglen, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_send(msg={:?}, msglen={}, answer={:?}, anslen={}) -> -1", msg, msglen, answer, anslen);
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn res_nsend(
        statep: *mut libc::c_void,
        msg: *const libc::c_char,
        msglen: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_nsend(ctx) {
        crate::strace!("res_nsend(statep={:?}, msg={:?}, msglen={}, answer={:?}, anslen={}) -> ...", statep, msg, msglen, answer, anslen);
        let msg_slice = slice::from_raw_parts(msg.cast::<u8>(), msglen as usize);

        match Scheduler::handle_event(&mut ctx, DnsResolveEvent::new(msg_slice)) {
            Ok(response) => {
                let len = cmp::min(response.len(), anslen as usize);
                let answer_slice = slice::from_raw_parts_mut(answer.cast::<u8>(), anslen as usize);
                answer_slice[..len].copy_from_slice(&response[..len]);
                crate::strace!("res_nsend(statep={:?}, msg={:?}, msglen={}, answer={:?}, anslen={}) -> {}", statep, msg, msglen, answer, anslen, len);
                len as libc::c_int // TODO: correct behavior on truncation?
            }
            Err(e) => {
                crate::strace!("res_nsend(statep={:?}, msg={:?}, msglen={}, answer={:?}, anslen={}) -> -1", statep, msg, msglen, answer, anslen);
                -1
            }
        }
    }
}
