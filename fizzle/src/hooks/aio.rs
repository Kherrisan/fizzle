use crate::hook_macros;
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn aio_read(
        _aiocbp: *mut libc::aiocb
    ) -> libc::ssize_t => fizzle_aio_read(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function aio_read() called within signal handler")
            }
        }

        unimplemented!("aio_read")
    }
}

hook_macros::hook! {
    unsafe fn aio_write(
        _aiocbp: *mut libc::aiocb
    ) -> libc::ssize_t => fizzle_aio_write(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function aio_write() called within signal handler")
            }
        }

        unimplemented!("aio_write")
    }
}

hook_macros::hook! {
    unsafe fn aio_fsync(
        _op: libc::c_int,
        _aiocbp: *mut libc::aiocb
    ) -> libc::ssize_t => fizzle_aio_fsync(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function aio_fsync() called within signal handler")
            }
        }
        unimplemented!("aio_fsync")
    }
}

hook_macros::hook! {
    unsafe fn aio_error(
        _aiocbp: *mut libc::aiocb
    ) -> libc::ssize_t => fizzle_aio_error(_ctx) {
        unimplemented!("aio_error")
    }
}

hook_macros::hook! {
    unsafe fn aio_return(
        _aiocbp: *mut libc::aiocb
    ) -> libc::ssize_t => fizzle_aio_return(_ctx) {
        unimplemented!("aio_return")
    }
}

hook_macros::hook! {
    unsafe fn aio_suspend(
        _aiocb_list: *const *const libc::aiocb,
        _nitems: libc::c_int,
        _timeout: *const libc::timespec
    ) -> libc::ssize_t => fizzle_aio_suspend(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function aio_suspend() called within signal handler")
            }
        }

        unimplemented!("aio_suspend")
    }
}

hook_macros::hook! {
    unsafe fn aio_cancel(
        _fd: libc::c_int,
        _aiocbp: *mut libc::aiocb
    ) -> libc::ssize_t => fizzle_aio_cancel(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function aio_cancel() called within signal handler")
            }
        }
        unimplemented!("aio_cancel")
    }
}

hook_macros::hook! {
    unsafe fn lio_listio(
        _mode: libc::c_int,
        _aiocb_list: *mut *mut libc::aiocb,
        _nitems: libc::c_int,
        _sevp: *mut libc::sigevent
    ) -> libc::ssize_t => fizzle_lio_listio(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function lio_listio() called within signal handler")
            }
        }

        unimplemented!("lio_listio")
    }
}
