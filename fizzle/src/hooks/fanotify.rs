use std::ffi::CStr;

use crate::hook_macros;
use crate::errno::Errno;

hook_macros::hook! {
    unsafe fn fanotify_init(
        flags: libc::c_uint,
        event_f_flags: libc::c_uint
    ) -> libc::c_int => fizzle_fanotify_init(_ctx) {
        crate::strace!("fanotify_init(flags={}, event_f_flags={}) -> ...", flags, event_f_flags);

        log::warn!("`fanotify_init()` unimplemented");
        let res = libc::fanotify_init(flags, event_f_flags);

        if res < 0 {
            let e = Errno::get_errno();
            crate::strace!("fanotify_init(flags={}, event_f_flags={}) -> -1 ({})", flags, event_f_flags, e);
            e.set_errno();
        } else {
            crate::strace!("fanotify_init(flags={}, event_f_flags={}) -> {}", flags, event_f_flags, res);
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn fanotify_mark(
        fanotify_fd: libc::c_int,
        flags: libc::c_uint,
        mask: u64,
        dirfd: libc::c_int,
        pathname: *const libc::c_char
    ) -> libc::c_int => fizzle_fanotify_mark(_ctx) {
        let path_cstr = if pathname.is_null() {
            None
        } else {
            Some(CStr::from_ptr(pathname))
        };

        crate::strace!("fanotify_mark(fanotify_fd={}, flags={}, mask={}, dirfd={}, pathname={:?}) -> ...", fanotify_fd, flags, mask, dirfd, path_cstr);

        log::warn!("`fanotify_mark()` unimplemented");
        let res = libc::fanotify_mark(fanotify_fd, flags, mask, dirfd, pathname);

        if res < 0 {
            let e = Errno::get_errno();
            crate::strace!("fanotify_mark(fanotify_fd={}, flags={}, mask={}, dirfd={}, pathname={:?}) -> -1 ({})", fanotify_fd, flags, mask, dirfd, path_cstr, e);
            e.set_errno();
        } else {
            crate::strace!("fanotify_mark(fanotify_fd={}, flags={}, mask={}, dirfd={}, pathname={:?}) -> 0", fanotify_fd, flags, mask, dirfd, path_cstr);
        }

        res
    }
}
