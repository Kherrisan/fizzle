use crate::hook_macros;

hook_macros::hook! {
    unsafe fn mq_open(
        _name: *const libc::c_char,
        _oflag: libc::c_int,
        _mode: libc::mode_t,
        _attr: *mut libc::mq_attr
    ) -> libc::mqd_t => fizzle_mq_open(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_close(
        _mqdes: libc::mqd_t
    ) -> libc::c_int => fizzle_mq_close(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_notify(
        _mqdes: libc::mqd_t,
        _sevp: *const libc::sigevent
    ) -> libc::c_int => fizzle_mq_notify(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_getattr(
        _mqdes: libc::mqd_t,
        _attr: *mut libc::mq_attr
    ) -> libc::c_int => fizzle_mq_getattr(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_setattr(
        _mqdes: libc::mqd_t,
        _newattr: *const libc::mq_attr,
        _oldattr: *mut libc::mq_attr
    ) -> libc::c_int => fizzle_mq_setattr(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_receive(
        _mqdes: libc::mqd_t,
        _msg_ptr: *mut libc::c_char,
        _msg_len: libc::size_t,
        _msg_prio: *mut libc::c_uint
    ) -> libc::c_int => fizzle_mq_receive(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_timedreceive(
        _mqdes: libc::mqd_t,
        _msg_ptr: *mut libc::c_char,
        _msg_len: libc::size_t,
        _msg_prio: *mut libc::c_uint,
        _abs_timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_mq_timedreceive(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_send(
        _mqdes: libc::mqd_t,
        _msg_ptr: *const libc::c_char,
        _msg_len: libc::size_t,
        _msg_prio: libc::c_uint
    ) -> libc::c_int => fizzle_mq_send(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_timedsend(
        _mqdes: libc::mqd_t,
        _msg_ptr: *const libc::c_char,
        _msg_len: libc::size_t,
        _msg_prio: libc::c_uint,
        _abs_timeout: *const libc::timespec
    ) -> libc::c_int => fizzle_mq_timedsend(_ctx) {
        unimplemented!()
    }
}

hook_macros::hook! {
    unsafe fn mq_unlink(
        _name: *const libc::c_char
    ) -> libc::c_int => fizzle_mq_unlink(_ctx) {
        unimplemented!()
    }
}
