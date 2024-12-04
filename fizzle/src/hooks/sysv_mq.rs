use crate::hook_macros;

hook_macros::hook! {
    unsafe fn msgget(
        _key: libc::key_t,
        _msgflg: libc::c_int
    ) -> libc::c_int => fizzle_msgget(_ctx) {
        unimplemented!("msgget")
    }
}

hook_macros::hook! {
    unsafe fn msgctl(
        _msqid: libc::c_int,
        _cmd: libc::c_int,
        _buf: *mut libc::msqid_ds
    ) -> libc::c_int => fizzle_msgctl(_ctx) {
        unimplemented!("msgctl")
    }
}

hook_macros::hook! {
    unsafe fn msgsnd(
        _msqid: libc::c_int,
        _msgp: *const libc::c_void,
        _msgsz: libc::size_t,
        _msgflg: libc::c_int
    ) -> libc::c_int => fizzle_msgsnd(_ctx) {
        unimplemented!("msgsnd")
    }
}

hook_macros::hook! {
    unsafe fn msgrcv(
        _msqid: libc::c_int,
        _msgp: *const libc::c_void,
        _msgsz: libc::size_t,
        _msgtyp: libc::c_long,
        _msgflg: libc::c_int
    ) -> libc::c_int => fizzle_msgrcv(_ctx) {
        unimplemented!("msgrcv")
    }
}
