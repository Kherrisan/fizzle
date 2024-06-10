use crate::hook_macros;

hook_macros::hook! {
    unsafe fn mq_open(
        name: *const libc::c_char,
        oflag: libc::c_int,
        mode: libc::mode_t,
        attr: *mut libc::mq_attr
    ) -> libc::mqd_t => fizzle_mq_open(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_close(
        mqdes: libc::mqd_t
    ) -> libc::c_int => fizzle_mq_close(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_notify(
        mqdes: libc::mqd_t,
        sevp: *const libc::sigevent
    ) -> libc::c_int => fizzle_mq_notify(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_getattr(
        mqdes: libc::mqd_t,
        attr: *mut libc::mq_attr
    ) -> libc::c_int => fizzle_mq_getattr(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_setattr(
        mqdes: libc::mqd_t,
        newattr: *const libc::mq_attr,
        oldattr: *mut libc::mq_attr
    ) -> libc::c_int => fizzle_mq_setattr(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_receive(
        mqdes: libc::mqd_t,
        msg_ptr: *mut libc::c_char,
        msg_len: libc::size_t,
        msg_prio: *mut libc::c_uint
    ) -> libc::c_int => fizzle_mq_receive(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_timedreceive(
        mqdes: libc::mqd_t,
        msg_ptr: *mut libc::c_char,
        msg_len: libc::size_t,
        msg_prio: *mut libc::c_uint,
        abs_timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_mq_timedreceive(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_send(
        mqdes: libc::mqd_t,
        msg_ptr: *const libc::c_char,
        msg_len: libc::size_t,
        msg_prio: libc::c_uint
    ) -> libc::c_int => fizzle_mq_send(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_timedsend(
        mqdes: libc::mqd_t,
        msg_ptr: *const libc::c_char,
        msg_len: libc::size_t,
        msg_prio: libc::c_uint,
        abs_timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_mq_timedsend(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_unlink(
        name: *const libc::c_char
    ) -> libc::c_int => fizzle_mq_unlink(_ctx) {
        unimplemented!()
    }
}
