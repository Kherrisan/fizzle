use std::ffi::CStr;

use fizzle_common::path::FilePath;

use crate::errno::Errno;
use crate::handlers::file::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;
use crate::strace;
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn lseek(
        fd: libc::c_int,
        offset: libc::off_t,
        whence: libc::c_int
    ) -> libc::off_t => fizzle_lseek(_ctx) {
        log::error!("unimplemented function `lseek`");

        return unsafe { libc::lseek(fd, offset, whence) };
    }
}

hook_macros::hook! {
    unsafe fn lseek64(
        fd: libc::c_int,
        offset: libc::off64_t,
        whence: libc::c_int
    ) -> libc::off64_t => fizzle_lseek64(_ctx) {
        log::error!("unimplemented function `lseek64`");

        return unsafe { libc::lseek64(fd, offset, whence) };
    }
}

hook_macros::hook! {
    unsafe fn umask(
        mask: libc::mode_t
    ) -> libc::c_uint => fizzle_umask(ctx) {
        let access_mode = AccessMode::from_bits_truncate(mask);

        crate::strace!("umask(mask={}) -> ...", access_mode);

        #[cfg(feature = "passthroughfs")] {
            let res = unsafe { libc::umask(mask) };
            crate::strace!("umask(mask={}) -> {}", access_mode, res);
            return res
        }

        // TODO: needs to return the previous mask
        match Scheduler::handle_event(&mut ctx, UmaskEvent::new(access_mode)) {
            Ok(prev) => {
                crate::strace!("umask(mask={}) -> {}", access_mode, prev);
                prev.bits()
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn open(
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_open(ctx) {
        crate::strace!("open(pathname={:?}, flags={:?}, mode={:?}) -> ...", pathname, flags, mode);

        let path_cstr = CStr::from_ptr(pathname);
        if path_cstr == c"/dev/random" || path_cstr == c"/dev/urandom" {
            log::info!("open() random /dev accessed--passing null bytes...");
            return unsafe { libc::open(c"/dev/zero".as_ptr(), flags, mode) };
        }

        let Some(open_flags) = FileOpenFlags::from_bits(flags) else {
            log::warn!("unrecognized flags in `open()`");
            strace!("open(pathname={:?}, flags={}) -> -1 (EINVAL)", path_cstr, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        #[cfg(feature = "passthroughfs")] {
            let res = if open_flags.contains(FileOpenFlags::CREATE) {
                libc::open(pathname, flags, mode)
            } else {
                libc::open(pathname, flags)
            };
            if res < 0 {
                strace!("open(path_cstr={path_cstr:?}, flags={open_flags:?}) -> -1 ({})", Errno::get_errno());
            } else {
                strace!("open(path_cstr={path_cstr:?}, flags={open_flags:?}) -> {res}");
            }

            return res
        }

        // TODO: track atime

        // TODO: deal with terminal devices

        // TODO: what about O_TRUNC?

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            log::warn!("malformed or oversized filepath passed to `open()`");
            strace!("open(pathname={:?}, flags={:?}) -> -1 (EINVAL)", pathname, open_flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let mode = if open_flags.contains(FileOpenFlags::CREATE) {
            // TODO: just ignores unrecognized open flag bits--correct??
            Some(AccessMode::from_bits_truncate(mode))
        } else {
            None
        };

        match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(relative_path.clone()), open_flags, mode)) {
            Ok(fd) => {
                crate::strace!("open(pathname={:?}, flags={:?}, mode={:?}) -> {}", relative_path, open_flags, mode, fd);
                fd
            },
            Err(e) => {
                crate::strace!("open(pathname={:?}, flags={:?}, mode={:?}) -> -1 ({})", relative_path, open_flags, mode, e);
                e.set_errno();
                -1
            }
        }
    }
}

// By default, files made with creat of O_CREAT will be created in the virtual fs.
hook_macros::hook! {
    unsafe fn creat(
        pathname: *const libc::c_char,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_creat(ctx) {
        let open_flags = FileOpenFlags::CREATE | FileOpenFlags::TRUNC | FileOpenFlags::WRITEONLY;

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::creat(pathname, mode) };

        // TODO: track atime

        // TODO: deal with terminal devices

        // TODO: what about O_TRUNC?

        let mode = AccessMode::from_bits_truncate(mode);

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            log::warn!("malformed or oversized filepath passed to `creat()`");
            strace!("creat(pathname={:?}, mode={:?}) -> -1 (EINVAL)", pathname, mode);
            Errno::EINVAL.set_errno();
            return -1
        };

        crate::strace!("creat(pathname={:?}, mode={:?}) -> ...", relative_path, mode);

        match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::Path(relative_path.clone()), open_flags, Some(mode))) {
            Ok(fd) => {
                crate::strace!("creat(pathname={:?}, mode={:?}) -> {}", relative_path, mode, fd);
                fd
            },
            Err(e) => {
                crate::strace!("creat(pathname={:?}, mode={:?}) -> -1 ({})", relative_path, mode, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn openat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_openat(ctx) {

        let path_cstr = CStr::from_ptr(pathname);
        if path_cstr == c"/dev/random" || path_cstr == c"/dev/urandom" {
            log::info!("openat() random /dev accessed--passing null bytes...");
            return unsafe { libc::openat(dirfd, c"/dev/zero".as_ptr(), flags, mode) };
        }

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::openat(dirfd, pathname, flags, mode) };

        let Some(open_flags) = FileOpenFlags::from_bits(flags) else {
            log::warn!("unrecognized flags in `openat()`");
            strace!("openat(dirfd={}, pathname={:?}, flags={}) -> -1 (EINVAL)", dirfd, pathname, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        // TODO: track atime

        // TODO: deal with terminal devices

        // TODO: what about O_TRUNC?

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            log::warn!("malformed or oversized filepath passed to `openat()`");
            strace!("openat(dirfd={}, pathname={:?}, flags={:?}) -> -1 (EINVAL)", dirfd, pathname, open_flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let mode = if open_flags.contains(FileOpenFlags::CREATE) {
            // TODO: just ignores unrecognized open flag bits--correct??
            Some(AccessMode::from_bits_truncate(mode))
        } else {
            None
        };

        crate::strace!("openat(dirfd={}, pathname={:?}, flags={:?}, mode={:?}) -> ...", dirfd, relative_path, open_flags, mode);

        match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(FileOpenLocation::PathAt(relative_path.clone(), dirfd), open_flags, mode)) {
            Ok(fd) => {
                crate::strace!("openat(dirfd={}, pathname={:?}, flags={:?}, mode={:?}) -> {}", dirfd, relative_path, open_flags, mode, fd);
                fd
            },
            Err(e) => {
                crate::strace!("openat(dirfd={}, pathname={:?}, flags={:?}, mode={:?}) -> -1 ({})", dirfd, relative_path, open_flags, mode, e);
                e.set_errno();
                -1
            }
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct file_handle {
    #[allow(unused)]
    pub handle_bytes: libc::c_uint,
    #[allow(unused)]
    pub handle_typ: libc::c_int,
    #[allow(unused)]
    pub f_handle: *mut libc::c_char,
}

hook_macros::hook! {
    unsafe fn name_to_handle_at(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        handle: *mut file_handle,
        mount_id: *mut libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_name_to_handle_at(_ctx) {
        log::warn!("`name_to_handle_at` not implemented by Fizzle");
        hook_macros::real!(name_to_handle_at)(dirfd, pathname, handle, mount_id, flags)
    }
}

hook_macros::hook! {
    unsafe fn open_by_handle_at(
        mount_fd: libc::c_int,
        handle: *mut file_handle,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_open_by_handle_at(_ctx) {
        log::warn!("`open_by_handle_at` not implemented by Fizzle");
        hook_macros::real!(open_by_handle_at)(mount_fd, handle, flags)
    }
}

hook_macros::hook! {
    unsafe fn chdir(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_chdir(ctx) {
        strace!("chdir(path={:?}) -> ...", path);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::chdir(path) };

        let Ok(filepath) = FilePath::from_cstr(CStr::from_ptr(path)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        match Scheduler::handle_event(&mut ctx, ChangeDirectoryEvent::new(ChangeDirectorySource::Path(filepath.clone()))) {
            Ok(()) => {
                strace!("chdir(path={:?}) -> 0", filepath);
                0
            },
            Err(e) => {
                strace!("chdir(path={:?}) -> -1 ({})", filepath, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fchdir(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_fchdir(ctx) {
        strace!("fchdir(fd={}) -> ...", fd);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fchdir(fd) };

        match Scheduler::handle_event(&mut ctx, ChangeDirectoryEvent::new(ChangeDirectorySource::Directory(fd))) {
            Ok(()) => {
                strace!("fchdir(fd={}) -> 0", fd);
                0
            },
            Err(e) => {
                strace!("fchdir(fd={}) -> -1 ({})", fd, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn chroot(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_chroot(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::chroot(path) };

        panic!("`chroot` not implemented for fizzle virtual fs")
    }
}

// Don't likely need to handle in any meaningful way (other than checking file existence):

hook_macros::hook! {
    unsafe fn chown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_chown(ctx) {
        strace!("chown(pathname={:?}, owner={}, group={}) -> ...", pathname, owner, group);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::chown(pathname, owner, group) };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, ChangeOwnerEvent::new(ChangeOwnerSource::Path(relative_path), owner, group, ChangeOwnerFlags::empty())) {
            Ok(()) => {
                strace!("chown(pathname={:?}, owner={}, group={}) -> 0", pathname, owner, group);
                0
            },
            Err(e) => {
                strace!("chown(pathname={:?}, owner={}, group={}) -> -1 ({})", pathname, owner, group, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn chmod(
        pathname: *const libc::c_char,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_chmod(ctx) {
        strace!("chmod(pathname={:?}, mode={}) -> ...", pathname, mode);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::chmod(pathname, mode) };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            strace!("chmod(pathname={:?}, mode={}) -> -1 (EINVAL)", pathname, mode);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, ChangeModeEvent::new(ChangeModeSource::Path(relative_path), mode, ChangeModeFlags::empty())) {
            Ok(()) => {
                strace!("chmod(pathname={:?}, mode={}) -> 0", pathname, mode);
                0
            },
            Err(e) => {
                strace!("chmod(pathname={:?}, mode={}) -> -1 ({})", pathname, mode, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fchown(
        fd: libc::c_int,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_fchown(ctx) {
        strace!("fchown(fd={}, owner={}, group={}) -> ...", fd, owner, group);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fchown(fd, owner, group) };

        match Scheduler::handle_event(&mut ctx, ChangeOwnerEvent::new(ChangeOwnerSource::Descriptor(fd), owner, group, ChangeOwnerFlags::empty())) {
            Ok(()) => {
                strace!("fchown(fd={:?}, owner={}, group={}) -> 0", fd, owner, group);
                0
            },
            Err(e) => {
                strace!("fchown(fd={:?}, owner={}, group={}) -> -1 ({})", fd, owner, group, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fchmod(
        fd: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_fchmod(ctx) {
        strace!("fchmod(fd={:?}, mode={}) -> ...", fd, mode);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fchmod(fd, mode) };

        match Scheduler::handle_event(&mut ctx, ChangeModeEvent::new(ChangeModeSource::Descriptor(fd), mode, ChangeModeFlags::empty())) {
            Ok(()) => {
                strace!("fchmod(fd={}, mode={}) -> 0", fd, mode);
                0
            },
            Err(e) => {
                strace!("fchmod(fd={}, mode={}) -> -1 ({})", fd, mode, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fchownat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_fchownat(ctx) {
        strace!("fchownat(dirfd={}, pathname={:?}, owner={}, group={}, flags={}) -> ...", dirfd, pathname, owner, group, flags);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fchownat(dirfd, pathname, owner, group, flags) };

        let Some(chown_flags) = ChangeOwnerFlags::from_bits(flags) else {
            strace!("fchownat(dirfd={}, pathname={:?}, owner={}, group={}, flags={}) -> -1 (EINVAL)", dirfd, pathname, owner, group, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            strace!("fchownat(dirfd={}, pathname={:?}, owner={}, group={}, flags={}) -> -1 (EINVAL)", dirfd, pathname, owner, group, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, ChangeOwnerEvent::new(ChangeOwnerSource::Path(relative_path), owner, group, chown_flags)) {
            Ok(()) => {
                strace!("fchownat(dirfd={}, pathname={:?}, owner={}, group={}, flags={:?}) -> 0", dirfd, pathname, owner, group, flags);
                0
            },
            Err(e) => {
                strace!("chown(dirfd={}, pathname={:?}, owner={}, group={}, flags={:?}) -> -1 ({})", dirfd, pathname, owner, group, flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn fchmodat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        mode: libc::mode_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_fchmodat(ctx) {
        strace!("fchmodat(dirfd={:?}, pathname={:?}, mode={}, flags={}) -> ...", dirfd, pathname, mode, flags);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fchmodat(dirfd, pathname, mode, flags) };

        let Some(chmod_flags) = ChangeModeFlags::from_bits(flags) else {
            strace!("fchmodat(dirfd={:?}, pathname={:?}, mode={}, flags={}) -> -1 (EINVAL)", dirfd, pathname, mode, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            strace!("fchmodat(dirfd={:?}, pathname={:?}, mode={}, flags={}) -> -1 (EINVAL)", dirfd, pathname, mode, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, ChangeModeEvent::new(ChangeModeSource::PathAt(relative_path, dirfd), mode, chmod_flags)) {
            Ok(()) => {
                strace!("fchmodat(dirfd={:?}, pathname={:?}, mode={}, flags={:?}) -> 0", dirfd, pathname, mode, chmod_flags);
                0
            },
            Err(e) => {
                strace!("fchmodat(dirfd={:?}, pathname={:?}, mode={}, flags={:?}) -> -1 ({})", dirfd, pathname, mode, chmod_flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn lchown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_lchown(ctx) {
        strace!("lchown(pathname={:?}, owner={}, group={}) -> ...", pathname, owner, group);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::lchown(pathname, owner, group) };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            strace!("lchown(pathname={:?}, owner={}, group={}) -> -1 (EINVAL)", pathname, owner, group);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, ChangeOwnerEvent::new(ChangeOwnerSource::Path(relative_path), owner, group, ChangeOwnerFlags::AT_SYMLINK_NOFOLLOW)) {
            Ok(()) => {
                strace!("lchown(pathname={:?}, owner={}, group={}) -> 0", pathname, owner, group);
                0
            },
            Err(e) => {
                strace!("lchown(pathname={:?}, owner={}, group={}) -> -1 ({})", pathname, owner, group, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn access(
        pathname: *mut libc::c_char,
        mode: libc::c_int
    ) -> libc::c_int => fizzle_access(ctx) {
        strace!("access(pathname={:?}, mode={}) -> ...", pathname, mode);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::access(pathname, mode) };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            strace!("access(pathname={:?}, mode={}) -> -1 (EINVAL)", pathname, mode);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, AccessEvent::new(AccessSource::Path(relative_path), mode, AccessFlags::empty())) {
            Ok(()) => {
                strace!("access(pathname={:?}, mode={}) -> 0", pathname, mode);
                0
            },
            Err(e) => {
                strace!("access(pathname={:?}, mode={}) -> -1 ({})", pathname, mode, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn faccessat(
        dirfd: libc::c_int,
        pathname: *mut libc::c_char,
        mode: libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_faccessat(ctx) {
        strace!("faccessat(dirfd={}, pathname={:?}, mode={}, flags={}) -> ...", dirfd, pathname, mode, flags);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::faccessat(dirfd, pathname, mode, flags) };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            strace!("faccessat(dirfd={}, pathname={:?}, mode={}, flags={}) -> -1 (EINVAL)", dirfd, pathname, mode, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let Some(access_flags) = AccessFlags::from_bits(flags) else {
            panic!("access flags unrecognized for `faccessat`")
        };

        match Scheduler::handle_event(&mut ctx, AccessEvent::new(AccessSource::PathAt(relative_path, dirfd), mode, access_flags)) {
            Ok(()) => {
                strace!("faccessat(dirfd={}, pathname={:?}, mode={}, flags={}) -> 0", dirfd, pathname, mode, flags);
                0
            },
            Err(e) => {
                strace!("faccessat(dirfd={}, pathname={:?}, mode={}, flags={}) -> -1 ({})", dirfd, pathname, mode, flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn truncate(
        path: *const libc::c_char,
        length: libc::off_t
    ) -> libc::c_int => fizzle_truncate(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::truncate(path, length) };

        unimplemented!("truncate()") // TODO: can implement now
    }
}

hook_macros::hook! {
    unsafe fn ftruncate(
        fd: libc::c_int,
        length: libc::off_t
    ) -> libc::c_int => fizzle_ftruncate(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::ftruncate(fd, length) };

        unimplemented!("ftruncate()") // TODO: can implement now
    }
}

hook_macros::hook! {
    unsafe fn sync() => fizzle_sync(_ctx) {
        log::warn!("`sync` unimplemented by Fizzle");
    }
}

hook_macros::hook! {
    unsafe fn syncfs(_fd: libc::c_int) -> libc::c_int => fizzle_syncfd(_ctx) {
        log::warn!("`syncfs` unimplemented by Fizzle");
        0
    }
}

hook_macros::hook! {
    unsafe fn fsync(
        _fd: libc::c_int
    ) -> libc::c_int => fizzle_fsync(_ctx) {
        log::warn!("`fsync` unimplemented by Fizzle");
        0
    }
}

hook_macros::hook! {
    unsafe fn fdatasync(
        _fd: libc::c_int
    ) -> libc::c_int => fizzle_fdatasync(_ctx) {
        log::warn!("`fdatasync` unimplemented by Fizzle");
        0
    }
}

hook_macros::hook! {
    unsafe fn readlink(
        pathname: *mut libc::c_char,
        buf: *mut libc::c_char,
        bufsiz: libc::size_t
    ) -> libc::ssize_t => fizzle_readlink(_ctx) {
        log::warn!("`readlink` unimplemented by Fizzle");
        hook_macros::real!(readlink)(pathname, buf, bufsiz)
    }
}

hook_macros::hook! {
    unsafe fn readlinkat(
        dirfd: libc::c_int,
        pathname: *mut libc::c_char,
        buf: *mut libc::c_char,
        bufsiz: libc::size_t
    ) -> libc::ssize_t => fizzle_readlinkat(_ctx) {
        log::warn!("`readlinkat` unimplemented by Fizzle");
        hook_macros::real!(readlinkat)(dirfd, pathname, buf, bufsiz)
    }
}

hook_macros::hook! {
    unsafe fn symlink(
        target: *mut libc::c_char,
        linkpath: *const libc::c_char
    ) -> libc::c_int => fizzle_symlink(_ctx) {
        log::warn!("`symlink` unimplemented by Fizzle");
        hook_macros::real!(symlink)(target, linkpath)
    }
}

hook_macros::hook! {
    unsafe fn symlinkat(
        target: *mut libc::c_char,
        newdirfd: libc::c_int,
        linkpath: *const libc::c_char
    ) -> libc::c_int => fizzle_symlinkat(_ctx) {
        log::warn!("`symlinkat` unimplemented by Fizzle");
        hook_macros::real!(symlinkat)(target, newdirfd, linkpath)
    }
}

hook_macros::hook! {
    unsafe fn link(
        oldpath: *mut libc::c_char,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_link(_ctx) {
        log::warn!("`link` unimplemented by Fizzle");
        hook_macros::real!(link)(oldpath, newpath)
    }
}

hook_macros::hook! {
    unsafe fn linkat(
        olddirfd: libc::c_int,
        oldpath: *mut libc::c_char,
        newdirfd: libc::c_int,
        newpath: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_linkat(_ctx) {
        log::warn!("`linkat` unimplemented by Fizzle");
        hook_macros::real!(linkat)(olddirfd, oldpath, newdirfd, newpath, flags)
    }
}

hook_macros::hook! {
    unsafe fn unlink(
        pathname: *const libc::c_char
    ) -> libc::c_int => fizzle_unlink(_ctx) {
        log::warn!("`unlink` unimplemented by Fizzle");
        hook_macros::real!(unlink)(pathname)
    }
}

hook_macros::hook! {
    unsafe fn unlinkat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_unlinkat(_ctx) {
        log::warn!("`unlinkat` unimplemented by Fizzle");
        hook_macros::real!(unlinkat)(dirfd, pathname, flags)
    }
}

hook_macros::hook! {
    unsafe fn rename(
        oldpath: *mut libc::c_char,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_rename(ctx) {
        strace!("rename(oldpath={:?}, newpath={:?}) -> ...", oldpath, newpath);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::rename(oldpath, newpath) };

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            strace!("rename(oldpath={:?}, newpath={:?}) -> -1 (EINVAL)", oldpath, newpath);
            Errno::EINVAL.set_errno();
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            strace!("rename(oldpath={:?}, newpath={:?}) -> -1 (EINVAL)", oldpath, newpath);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, RenameEvent::new(RenameSrcDst::Path(rel_oldpath.clone(), rel_newpath.clone()), RenameFlags::empty())) {
            Ok(()) => {
                strace!("rename(oldpath={:?}, newpath={:?}) -> 0", rel_oldpath, rel_newpath);
                0
            },
            Err(e) => {
                strace!("rename(oldpath={:?}, newpath={:?}) -> -1 ({})", rel_oldpath, rel_newpath, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn renameat(
        olddirfd: libc::c_int,
        oldpath: *mut libc::c_char,
        newdirfd: libc::c_int,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_renameat(ctx) {
        strace!("renameat(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}) -> ...", olddirfd, oldpath, newdirfd, newpath);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::renameat(olddirfd, oldpath, newdirfd, newpath) };

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            strace!("renameat(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}) -> -1 (EINVAL)", olddirfd, oldpath, newdirfd, newpath);
            Errno::EINVAL.set_errno();
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            strace!("renameat(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}) -> -1 (EINVAL)", olddirfd, oldpath, newdirfd, newpath);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, RenameEvent::new(RenameSrcDst::PathAt(rel_oldpath.clone(), olddirfd, rel_newpath.clone(), newdirfd), RenameFlags::empty())) {
            Ok(()) => {
                strace!("renameat(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}) -> 0", olddirfd, oldpath, newdirfd, newpath);
                0
            },
            Err(e) => {
                strace!("renameat(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}) -> -1 ({})", olddirfd, oldpath, newdirfd, newpath, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn renameat2(
        olddirfd: libc::c_int,
        oldpath: *mut libc::c_char,
        newdirfd: libc::c_int,
        newpath: *const libc::c_char,
        flags: libc::c_uint
    ) -> libc::c_int => fizzle_renameat2(ctx) {
        let rename_flags = RenameFlags::from_bits_truncate(flags);

        strace!("renameat2(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}, flags={:?}) -> ...", olddirfd, oldpath, newdirfd, newpath, rename_flags);

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::renameat2(olddirfd, oldpath, newdirfd, newpath, flags) };

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            strace!("renameat2(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}, flags={:?}) -> -1 (EINVAL)", olddirfd, oldpath, newdirfd, newpath, rename_flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            strace!("renameat2(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}, flags={:?}) -> -1 (EINVAL)", olddirfd, oldpath, newdirfd, newpath, rename_flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, RenameEvent::new(RenameSrcDst::PathAt(rel_oldpath.clone(), olddirfd, rel_newpath.clone(), newdirfd), rename_flags)) {
            Ok(()) => {
                strace!("renameat2(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}, flags={:?}) -> 0", olddirfd, oldpath, newdirfd, newpath, rename_flags);
                0
            },
            Err(e) => {
                strace!("renameat2(olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}, flags={:?}) -> -1 ({})", olddirfd, oldpath, newdirfd, newpath, rename_flags, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn mknod(
        pathname: *const libc::c_char,
        mode: libc::mode_t,
        dev: libc::dev_t
    ) -> libc::c_int => fizzle_mknod(_ctx) {
        log::warn!("`mknod` unimplemented by Fizzle");
        hook_macros::real!(mknod)(pathname, mode, dev)
    }
}

hook_macros::hook! {
    unsafe fn mknodat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        mode: libc::mode_t,
        dev: libc::dev_t
    ) -> libc::c_int => fizzle_mknodat(_ctx) {
        log::warn!("`mknodat` unimplemented by Fizzle");
        hook_macros::real!(mknodat)(dirfd, pathname, mode, dev)
    }
}

hook_macros::hook! {
    unsafe fn mount(
        source: *const libc::c_char,
        target: *const libc::c_char,
        filesystemtype: *const libc::c_char,
        mountflags: libc::c_ulong,
        data: *const libc::c_void
    ) -> libc::c_int => fizzle_mount(_ctx) {
        log::warn!("`mount` unimplemented by Fizzle");
        hook_macros::real!(mount)(source, target, filesystemtype, mountflags, data)
    }
}

hook_macros::hook! {
    unsafe fn umount(
        target: *const libc::c_char
    ) -> libc::c_int => fizzle_umount(_ctx) {
        log::warn!("`umount` unimplemented by Fizzle");
        hook_macros::real!(umount)(target)
    }
}

hook_macros::hook! {
    unsafe fn umount2(
        target: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_umount2(_ctx) {
        log::warn!("`umount2` unimplemented by Fizzle");
        hook_macros::real!(umount2)(target, flags)
    }
}
