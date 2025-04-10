use std::ffi::CStr;

use fizzle_common::path::FilePath;

use crate::{errno::Errno, handlers::file::{StatEvent, StatFlags, StatSource}, hook_macros, scheduler::Scheduler};


hook_macros::hook! {
    unsafe fn stat(
        pathname: *mut libc::c_char,
        statbuf: *mut libc::stat
    ) -> libc::c_int => fizzle_stat(ctx) {
        crate::strace!("stat(pathname={:?}, statbuf={:?}) -> ...", pathname, statbuf);

        #[cfg(feature = "passthroughfs")]
        unsafe {
            let res = libc::stat(pathname, statbuf);
            if res == 0 {
                (*statbuf).st_atime = 1735924847;
                (*statbuf).st_atime_nsec = 0;
                (*statbuf).st_ctime = 1735924847;
                (*statbuf).st_ctime_nsec = 0;
                (*statbuf).st_mtime = 1735924847;
                (*statbuf).st_mtime_nsec = 0;
            }

            return res
        }

        let stat_mut = statbuf.as_mut().unwrap();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            crate::strace!("stat(pathname={:?}, statbuf={:?}) -> -1 (EINVAL)", pathname, statbuf);
            Errno::EINVAL.set_errno();
            return -1
        };


        match Scheduler::handle_event(&mut ctx, StatEvent::new(StatSource::Path(relative_path), stat_mut, StatFlags::empty())) {
            Ok(()) => {
                crate::strace!("stat(pathname={:?}, statbuf={:?}) -> 0", pathname, statbuf);
                0
            },
            Err(e) => {
                crate::strace!("stat(pathname={:?}, statbuf={:?}) -> -1 ({})", pathname, statbuf, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn lstat(
        pathname: *mut libc::c_char,
        statbuf: *mut libc::stat
    ) -> libc::c_int => fizzle_lstat(ctx) {
        crate::strace!("lstat(pathname={:?}, statbuf={:?}) -> ...", pathname, statbuf);

        #[cfg(feature = "passthroughfs")]
        unsafe {
            let res = libc::lstat(pathname, statbuf);

            if res == 0 {
                (*statbuf).st_atime = 1735924847;
                (*statbuf).st_atime_nsec = 0;
                (*statbuf).st_ctime = 1735924847;
                (*statbuf).st_ctime_nsec = 0;
                (*statbuf).st_mtime = 1735924847;
                (*statbuf).st_mtime_nsec = 0;
            }

            return res
        }

        let stat_mut = statbuf.as_mut().unwrap();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            crate::strace!("lstat(pathname={:?}, statbuf={:?}) -> -1 (EINVAL)", pathname, statbuf);
            Errno::EINVAL.set_errno();
            return -1
        };


        match Scheduler::handle_event(&mut ctx, StatEvent::new(StatSource::Path(relative_path), stat_mut, StatFlags::AT_SYMLINK_NOFOLLOW)) {
            Ok(()) => {
                crate::strace!("lstat(pathname={:?}, statbuf={:?}) -> 0", pathname, statbuf);
                0
            },
            Err(e) => {
                crate::strace!("lstat(pathname={:?}, statbuf={:?}) -> -1 ({})", pathname, statbuf, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fstat(
        fd: libc::c_int,
        statbuf: *mut libc::stat
    ) -> libc::c_int => fizzle_fstat(ctx) {
        crate::strace!("fstat(fd={}, statbuf={:?}) -> ...", fd, statbuf);

        #[cfg(feature = "passthroughfs")]
        unsafe {
            let res = libc::fstat(fd, statbuf);

            if res == 0 {
                (*statbuf).st_atime = 1735924847;
                (*statbuf).st_atime_nsec = 0;
                (*statbuf).st_ctime = 1735924847;
                (*statbuf).st_ctime_nsec = 0;
                (*statbuf).st_mtime = 1735924847;
                (*statbuf).st_mtime_nsec = 0;
            }

            return res
        }

        let stat_mut = statbuf.as_mut().unwrap();

        match Scheduler::handle_event(&mut ctx, StatEvent::new(StatSource::Descriptor(fd), stat_mut, StatFlags::empty())) {
            Ok(()) => {
                crate::strace!("fstat(fd={}, statbuf={:?}) -> 0", fd, statbuf);
                0
            },
            Err(e) => {
                crate::strace!("fstat(fd={}, statbuf={:?}) -> -1 ({})", fd, statbuf, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fstatat(
        dirfd: libc::c_int,
        pathname: *mut libc::c_char,
        statbuf: *mut libc::stat,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_fstatat(ctx) {
        crate::strace!("fstatat(dirfd={}, pathname={:?}, statbuf={:?}, flags={}) -> ...", dirfd, pathname, statbuf, flags);

        #[cfg(feature = "passthroughfs")]
        unsafe {
            let res = libc::fstatat(dirfd, pathname, statbuf, flags);
            if res == 0 {
                (*statbuf).st_atime = 1735924847;
                (*statbuf).st_atime_nsec = 0;
                (*statbuf).st_ctime = 1735924847;
                (*statbuf).st_ctime_nsec = 0;
                (*statbuf).st_mtime = 1735924847;
                (*statbuf).st_mtime_nsec = 0;
            }

            return res
        }

        let stat_mut = statbuf.as_mut().unwrap();

        let stat_flags = StatFlags::from_bits_truncate(flags);

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            crate::strace!("fstatat(dirfd={}, pathname={:?}, statbuf={:?}, flags={}) -> -1 (EINVAL)", dirfd, pathname, statbuf, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, StatEvent::new(StatSource::PathAt(relative_path, dirfd), stat_mut, stat_flags)) {
            Ok(()) => {
                crate::strace!("fstatat(dirfd={}, pathname={:?}, statbuf={:?}, flags={}) -> 0", dirfd, pathname, statbuf, flags);
                0
            },
            Err(e) => {
                crate::strace!("fstatat(dirfd={}, pathname={:?}, statbuf={:?}, flags={}) -> -1 ({})", dirfd, pathname, statbuf, flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn statvfs(
        path: *mut libc::c_char,
        buf: *mut libc::statvfs
    ) -> libc::c_int => fizzle_statvfs(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::statvfs(path, buf) };

        unimplemented!("statvfs()")
    }
}

hook_macros::hook! {
    unsafe fn fstatvfs(
        fd: libc::c_int,
        buf: *mut libc::statvfs
    ) -> libc::c_int => fizzle_fstatvfs(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fstatvfs(fd, buf) };
        unimplemented!("fstatvfs()")
    }
}
