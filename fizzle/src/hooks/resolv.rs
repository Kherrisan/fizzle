use crate::hook_macros;


hook_macros::hook! {
    unsafe fn res_query(
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_query(ctx) {
        unimplemented!("res_query");
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
        unimplemented!("res_nquery");
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
        unimplemented!("res_search");
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
        unimplemented!("res_nsearch");
    }
}

hook_macros::hook! {
    unsafe fn res_send(
        statep: *mut libc::c_void,
        msg: *const libc::c_char,
        msglen: libc::c_int,
        answer: *mut libc::c_char,
        anslen: libc::c_int
    ) -> libc::c_int => fizzle_res_send(ctx) {
        unimplemented!("res_send");
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
        unimplemented!("res_nsend");
    }
}
