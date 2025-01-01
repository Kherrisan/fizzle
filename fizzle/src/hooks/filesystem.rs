use std::ffi::CStr;

use fizzle_common::path::FilePath;

use crate::backend::FileBackend;
use crate::errno::Errno;
use crate::handlers::descriptor::*;
use crate::handlers::file::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;
use crate::strace;

hook_macros::hook! {
    unsafe fn lseek(
        fd: libc::c_int,
        offset: libc::off_t,
        whence: libc::c_int
    ) -> libc::c_int => fizzle_lseek(_ctx) {
        hook_macros::real!(lseek)(fd, offset, whence)
    }
}

hook_macros::hook! {
    unsafe fn umask(
        mask: libc::mode_t
    ) -> libc::c_int => fizzle_umask(_ctx) {

        // TODO: set umask in virtual fs once permissions implemented
        log::error!("unimplemented function `umask`");

        hook_macros::real!(umask)(mask)
    }
}

hook_macros::hook! {
    unsafe fn open(
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_open(ctx) {
        let Some(open_flags) = FileOpenFlags::from_bits(flags) else {
            log::warn!("unrecognized flags in `open()`");
            strace!("open(pathname={:?}, flags={}) -> -1 (EINVAL)", pathname, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        // TODO: track atime

        // TODO: deal with terminals

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

        crate::strace!("open(pathname={:?}, flags={:?}, mode={:?}) -> ...", relative_path, open_flags, mode);

        match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(relative_path.clone(), open_flags, mode)) {
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

        // TODO: track atime

        // TODO: deal with terminals

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

        crate::strace!("open(pathname={:?}, flags={:?}, mode={:?}) -> ...", relative_path, open_flags, mode);

        match Scheduler::handle_event(&mut ctx, FileOpenEvent::new(relative_path.clone(), open_flags, mode)) {
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

hook_macros::hook! {
    unsafe fn openat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_openat(ctx) {
        let mut state = ctx.acquire();

        let close_on_exec = (flags & libc::O_CLOEXEC) != 0;
        // TODO: file locking is not yet supported here...

        // TODO: track atime

        // TODO: deal with terminals

        // TODO: what about O_TRUNC?

        let Ok(mut path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !path.is_absolute() {
            if dirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                path = cwd.clone().concat(&path).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(dirfd)).cloned() else {
                    log::debug!("`openat` called with unrecognized file descriptor");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(&dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                path = dir_path.clone().concat(&path).unwrap();
            }
        }

        // Files are drawn from the underlying filesystem by default.
        // A user may configure certain file paths to be mapped to virtual files.
        // Likewise, files created during the lifetime of fizzle are stored virtually.

        if (flags & libc::O_CREAT) != 0 {
            // TODO: we ignore open mode here

            let file_id = match state.global.create_file(path) {
                Ok(file_id) => file_id,
                Err(_) if (flags & libc::O_EXCL) != 0 => {
                    *libc::__errno_location() = libc::EEXIST;
                    return -1
                }
                Err(file_id) => file_id,
            };

            let fd = crate::create_descriptor();

            state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
                close_on_exec,
                nonblocking: false,
                is_passthrough: false,
                resource: FdResource::File(file_id)
            }).unwrap();

            fd

        } else if (flags & libc::O_PATH) != 0 {
            // TODO: what about O_CREAT here?
            let fd = hook_macros::real!(open)(pathname, flags, mode);
            let dir_id = state.local.dirs.allocate(path).unwrap();
            state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
                close_on_exec,
                nonblocking: false,
                is_passthrough: true,
                resource: FdResource::Directory(dir_id)
            }).unwrap();

            fd

        } else if let Some(file_id) = state.global.file_paths.get(&path).cloned() {
            let fd = crate::create_descriptor();
            state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
                close_on_exec,
                nonblocking: false,
                is_passthrough: true,
                resource: FdResource::File(file_id),
            }).unwrap();
            fd

        } else {
            let fd = hook_macros::real!(open)(pathname, flags, mode);
            if fd >= 0 {
                let file_id = state.global.files.allocate(FileBackend::Passthrough).unwrap();

                state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
                    close_on_exec: false,
                    nonblocking: false,
                    is_passthrough: true,
                    resource: FdResource::File(file_id),
                }).unwrap();
            }

            fd
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

        hook_macros::real!(name_to_handle_at)(dirfd, pathname, handle, mount_id, flags)
    }
}

hook_macros::hook! {
    unsafe fn open_by_handle_at(
        mount_fd: libc::c_int,
        handle: *mut file_handle,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_open_by_handle_at(_ctx) {

        hook_macros::real!(open_by_handle_at)(mount_fd, handle, flags)
    }
}

hook_macros::hook! {
    unsafe fn chdir(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_chdir(ctx) {
        let mut state = ctx.acquire();

        let res = hook_macros::real!(chdir)(path);

        if res == 0 {
            let Ok(new_abspath) = FilePath::from_cstr(CStr::from_ptr(path)) else {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            };

            state.local.working_directory = new_abspath;
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn fchdir(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_fchdir(ctx) {
        let mut state = ctx.acquire();

        let res = hook_macros::real!(fchdir)(fd);
        if res == 0 {
            let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(fd)) else {
                log::debug!("`fchdir` called with unrecognized fd");
                *libc::__errno_location() = libc::EBADF;
                return -1
            };

            let Some(path) = state.local.dirs.get(dir_id) else {
                panic!("inconsistent fizzle state in directory fds for `fchdir`");
            };

            state.local.working_directory = path.clone();
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn chroot(
        _path: *const libc::c_char
    ) -> libc::c_int => fizzle_chroot(_ctx) {

        crate::report_strict_failure("`chroot` not implemented for fizzle virtual fs");
        -1
    }
}

// Don't likely need to handle in any meaningful way (other than checking file existence):

hook_macros::hook! {
    unsafe fn chown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_chown(ctx) {
        let state = ctx.acquire();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(chown)(pathname, owner, group)
        }
    }
}

hook_macros::hook! {
    unsafe fn chmod(
        pathname: *const libc::c_char,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_chmod(ctx) {
        let state = ctx.acquire();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(chmod)(pathname, mode)
        }
    }
}

hook_macros::hook! {
    unsafe fn fchown(
        fd: libc::c_int,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_fchown(ctx) {
        let state = ctx.acquire();

        if let Some(_fd_info) = state.local.fds.get(&DescriptorId::from_raw_fd(fd)) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(fchown)(fd, owner, group)
        }
    }
}

hook_macros::hook! {
    unsafe fn fchmod(
        fd: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_fchmod(ctx) {
        let state = ctx.acquire();

        if let Some(_fd_info) = state.local.fds.get(&DescriptorId::from_raw_fd(fd)) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(fchmod)(fd, mode)
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
        let state = ctx.acquire();

        let Ok(mut path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !path.is_absolute() {
            if dirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                path = cwd.clone().concat(&path).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(dirfd)) else {
                    log::debug!("`fchownat` called with unrecognized file descriptor");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                path = dir_path.clone().concat(&path).unwrap();
            }
        }

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(fchownat)(dirfd, pathname, owner, group, flags)
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
        let state = ctx.acquire();

        let Ok(mut path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !path.is_absolute() {
            if dirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                path = cwd.clone().concat(&path).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(dirfd)) else {
                    log::debug!("`fchmodat` called with unrecognized file descriptor");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                path = dir_path.clone().concat(&path).unwrap();
            }
        }

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(fchmodat)(dirfd, pathname, mode, flags)
        }
    }
}

hook_macros::hook! {
    unsafe fn lchown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_lchown(ctx) {
        let state = ctx.acquire();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(lchown)(pathname, owner, group)
        }
    }
}

hook_macros::hook! {
    unsafe fn access(
        pathname: *mut libc::c_char,
        mode: libc::c_int
    ) -> libc::c_int => fizzle_access(ctx) {
        let state = ctx.acquire();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle passthrough
        } else {
            hook_macros::real!(access)(pathname, mode)
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
        let state = ctx.acquire();

        let Ok(mut path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !path.is_absolute() {
            if dirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                path = cwd.clone().concat(&path).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(dirfd)) else {
                    log::debug!("`faccessat` called with unrecognized file descriptor");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                path = dir_path.clone().concat(&path).unwrap();
            }
        }

        if state.global.file_paths.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(faccessat)(dirfd, pathname, mode, flags)
        }
    }
}

hook_macros::hook! {
    unsafe fn stat(
        pathname: *mut libc::c_char,
        statbuf: *mut libc::stat
    ) -> libc::c_int => fizzle_stat(ctx) {
        let state = ctx.acquire();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if state.global.file_paths.contains_key(&path) {
            crate::report_strict_failure("`stat` not implemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(stat)(pathname, statbuf)
        }
    }
}

hook_macros::hook! {
    unsafe fn lstat(
        pathname: *mut libc::c_char,
        statbuf: *mut libc::stat
    ) -> libc::c_int => fizzle_lstat(ctx) {
        let state = ctx.acquire();

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = state.local.working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if state.global.file_paths.contains_key(&path) {
            crate::report_strict_failure("`lstat` not implemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(lstat)(pathname, statbuf)
        }
    }
}

hook_macros::hook! {
    unsafe fn fstat(
        fd: libc::c_int,
        statbuf: *mut libc::stat
    ) -> libc::c_int => fizzle_fstat(ctx) {
        let state = ctx.acquire();

        if let Some(_fd_info) = state.local.fds.get(&DescriptorId::from_raw_fd(fd)) {
            crate::report_strict_failure("`fstat` not implemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(fstat)(fd, statbuf)
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
        let state = ctx.acquire();

        let Ok(mut path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !path.is_absolute() {
            if dirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                path = cwd.clone().concat(&path).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(dirfd)) else {
                    log::debug!("`fstatat` called with unrecognized file descriptor");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                path = dir_path.clone().concat(&path).unwrap();
            }
        }

        if state.global.file_paths.contains_key(&path) {
            crate::report_strict_failure("`fstatat` unimplemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(fstatat)(dirfd, pathname, statbuf, flags)
        }
    }
}

hook_macros::hook! {
    unsafe fn statvfs(
        _path: *mut libc::c_char,
        _buf: *mut libc::statvfs
    ) => fizzle_statvfs(_ctx) {
        unimplemented!("statvfs()")
    }
}

hook_macros::hook! {
    unsafe fn fstatvfs(
        _fd: libc::c_int,
        _buf: *mut libc::statvfs
    ) => fizzle_fstatvfs(_ctx) {
        unimplemented!("fstatvfs()")
    }
}

hook_macros::hook! {
    unsafe fn truncate(
        _path: *const libc::c_char,
        _length: libc::off_t
    ) => fizzle_truncate(_ctx) {
        unimplemented!("truncate()")
    }
}

hook_macros::hook! {
    unsafe fn ftruncate(
        _fd: libc::c_int,
        _length: libc::off_t
    ) => fizzle_ftruncate(_ctx) {
        unimplemented!("ftruncate()")
    }
}

hook_macros::hook! {
    unsafe fn fsync(
        _fd: libc::c_int
    ) => fizzle_fsync(_ctx) {
        unimplemented!("fsync()")
    }
}

hook_macros::hook! {
    unsafe fn fdatasync(
        _fd: libc::c_int
    ) => fizzle_fdatasync(_ctx) {
        unimplemented!("fdatasync()")
    }
}

hook_macros::hook! {
    unsafe fn readlink(
        pathname: *mut libc::c_char,
        buf: *mut libc::c_char,
        bufsiz: libc::size_t
    ) -> libc::c_int => fizzle_readlink(_ctx) {
        hook_macros::real!(readlink)(pathname, buf, bufsiz)
    }
}

hook_macros::hook! {
    unsafe fn readlinkat(
        dirfd: libc::c_int,
        pathname: *mut libc::c_char,
        buf: *mut libc::c_char,
        bufsiz: libc::size_t
    ) -> libc::c_int => fizzle_readlinkat(_ctx) {
        hook_macros::real!(readlinkat)(dirfd, pathname, buf, bufsiz)
    }
}

hook_macros::hook! {
    unsafe fn symlink(
        target: *mut libc::c_char,
        linkpath: *const libc::c_char
    ) -> libc::c_int => fizzle_symlink(_ctx) {
        hook_macros::real!(symlink)(target, linkpath)
    }
}

hook_macros::hook! {
    unsafe fn symlinkat(
        target: *mut libc::c_char,
        newdirfd: libc::c_int,
        linkpath: *const libc::c_char
    ) -> libc::c_int => fizzle_symlinkat(_ctx) {
        hook_macros::real!(symlinkat)(target, newdirfd, linkpath)
    }
}

hook_macros::hook! {
    unsafe fn link(
        oldpath: *mut libc::c_char,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_link(_ctx) {
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
        hook_macros::real!(linkat)(olddirfd, oldpath, newdirfd, newpath, flags)
    }
}

hook_macros::hook! {
    unsafe fn unlink(
        pathname: *const libc::c_char
    ) -> libc::c_int => fizzle_unlink(_ctx) {
        hook_macros::real!(unlink)(pathname)
    }
}

hook_macros::hook! {
    unsafe fn unlinkat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_unlinkat(_ctx) {
        hook_macros::real!(unlinkat)(dirfd, pathname, flags)
    }
}

hook_macros::hook! {
    unsafe fn rename(
        oldpath: *mut libc::c_char,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_rename(ctx) {
        let mut state = ctx.acquire();

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(abs_oldpath) = state.local.working_directory.clone().concat(&rel_oldpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(_abs_newpath) = state.local.working_directory.clone().concat(&rel_newpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        // TODO: handle inode deletion here
        if state.global.file_paths.remove(&abs_oldpath).is_some() {
            crate::report_strict_failure("`rename` not implemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(rename)(oldpath, newpath)
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
        let mut state = ctx.acquire();

        let Ok(mut old) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !old.is_absolute() {
            if olddirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                old = cwd.clone().concat(&old).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(olddirfd)) else {
                    log::debug!("`renameat` called with unrecognized file descriptor `olddirfd`");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                old = dir_path.clone().concat(&old).unwrap();
            }
        }

        let Ok(mut _new) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !_new.is_absolute() {
            if newdirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                _new = cwd.clone().concat(&_new).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(newdirfd)) else {
                    log::debug!("`renameat` called with unrecognized file descriptor `newdirfd`");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                _new = dir_path.clone().concat(&_new).unwrap();
            }
        }

        // TODO: handle inode deletion
        if state.global.file_paths.remove(&old).is_some() {
            crate::report_strict_failure("`renameat` not implemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(renameat)(olddirfd, oldpath, newdirfd, newpath)
        }
    }
}

hook_macros::hook! {
    unsafe fn renameat2(
        olddirfd: libc::c_int,
        oldpath: *mut libc::c_char,
        newdirfd: libc::c_int,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_renameat2(ctx) {
        let mut state = ctx.acquire();

        let Ok(mut old) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !old.is_absolute() {
            if olddirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                old = cwd.clone().concat(&old).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(olddirfd)) else {
                    log::debug!("`renameat2` called with unrecognized file descriptor `olddirfd`");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                old = dir_path.clone().concat(&old).unwrap();
            }
        }

        let Ok(mut _new) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if !_new.is_absolute() {
            if newdirfd == libc::AT_FDCWD {
                let cwd = &state.local.working_directory;
                _new = cwd.clone().concat(&_new).unwrap();
            } else {
                let Some(DescriptorInfo { resource: FdResource::Directory(dir_id), .. }) = state.local.fds.get(&DescriptorId::from_raw_fd(newdirfd)) else {
                    log::debug!("`renameat2` called with unrecognized file descriptor `newdirfd`");
                    *libc::__errno_location() = libc::ENOTDIR;
                    return -1
                };

                let Some(dir_path) = state.local.dirs.get(dir_id) else {
                    *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
                    return -1
                };

                _new = dir_path.clone().concat(&_new).unwrap();
            }
        }

        if state.global.file_paths.remove(&old).is_some() {
            crate::report_strict_failure("`renameat2` not implemented for fizzle virtual fs");
            -1
        } else {
            hook_macros::real!(renameat2)(olddirfd, oldpath, newdirfd, newpath)
        }
    }
}

hook_macros::hook! {
    unsafe fn mknod(
        pathname: *const libc::c_char,
        mode: libc::mode_t,
        dev: libc::dev_t
    ) -> libc::c_int => fizzle_mknod(_ctx) {
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
        hook_macros::real!(mount)(source, target, filesystemtype, mountflags, data)
    }
}

hook_macros::hook! {
    unsafe fn umount(
        target: *const libc::c_char
    ) -> libc::c_int => fizzle_umount(_ctx) {
        hook_macros::real!(umount)(target)
    }
}

hook_macros::hook! {
    unsafe fn umount2(
        target: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_umount2(_ctx) {
        hook_macros::real!(umount2)(target, flags)
    }
}
