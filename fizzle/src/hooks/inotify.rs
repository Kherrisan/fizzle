use std::ffi::CStr;

use crate::errno::Errno;
use crate::handlers::descriptor::Descriptor;
use crate::handlers::inotify::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn inotify_init() -> libc::c_int => fizzle_inotify_init(_ctx) {
        crate::strace!("inotify_init() -> ...");

        log::error!("`inotify_init()` unimplemented");
        let res = unsafe { libc::inotify_init() };

        if res < 0 {
            let e = Errno::get_errno();
            crate::strace!("inotify_init() -> -1 ({})", e);
            e.set_errno();
        } else {
            crate::strace!("inotify_init() -> {}", res);
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn inotify_init1(
        flags: libc::c_int
    ) -> libc::c_int => fizzle_inotify_init1(_ctx) {
        crate::strace!("inotify_init1(flags={}) -> ...", flags);

        log::error!("`inotify_init1()` unimplemented");
        let res = unsafe { libc::inotify_init1(flags) };

        if res < 0 {
            let e = Errno::get_errno();
            crate::strace!("inotify_init1(flags={}) -> -1 ({})", flags, e);
            e.set_errno();
        } else {
            crate::strace!("inotify_init(flags={}) -> {}", flags, res);
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn inotify_add_watch(
        fd: libc::c_int,
        pathname: *const libc::c_char,
        mask: u32
    ) -> libc::c_int => fizzle_inotify_add_watch(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);
        let path_cstr = CStr::from_ptr(pathname);
        let Some(inotify_events) = InotifyEvents::from_bits(mask) else {
            panic!("unsupported flags in inotify_add_watch")
        };

        crate::strace!("inotify_add_watch(fd={}, pathname={:?}, mask={:?}) -> ...", fd, path_cstr, inotify_events);

        match Scheduler::handle_event(&mut ctx, InotifyAddWatchEvent::new(descriptor_id, path_cstr, inotify_events)) {
            Ok(wd) => {
                crate::strace!("inotify_add_watch(fd={}, pathname={:?}, mask={:?}) -> {}", fd, path_cstr, inotify_events, wd);
                wd
            },
            Err(e) => {
                crate::strace!("inotify_add_watch(fd={}, pathname={:?}, mask={:?}) -> -1 ({})", fd, path_cstr, inotify_events, e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn inotify_rm_watch(
        fd: libc::c_int,
        wd: libc::c_int
    ) -> libc::c_int => fizzle_inotify_rm_watch(ctx) {
        let descriptor_id = Descriptor::from_raw_fd(fd);

        crate::strace!("inotify_rm_watch(fd={}, wd={}) -> ...", fd, wd);

        match Scheduler::handle_event(&mut ctx, InotifyRemoveWatchEvent::new(descriptor_id, wd)) {
            Ok(()) => {
                crate::strace!("inotify_rm_watch(fd={}, wd={}) -> 0", fd, wd);
                0
            },
            Err(e) => {
                crate::strace!("inotify_rm_watch(fd={}, wd={}) -> -1 ({})", fd, wd, e);
                e.set_errno();
                -1
            },
        }
    }
}
