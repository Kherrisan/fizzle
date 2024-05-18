use std::ffi::CStr;
use std::ptr;

use crate::state::fd::FdInfo;
use crate::{
    hook_macros,
    state::{self, FileId, FileInfo},
    FilePath,
};

hook_macros::hook! {
    unsafe fn fdopen(
        fd: libc::c_int,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fdopen(ctx) {

        if let Some(fd_info) = ctx.local().fds.get(&fd) {
            match fd_info {
                FdInfo::File(_) | FdInfo::PassthroughFile(_) => (),
                _ => {
                    *libc::__errno_location() = libc::EBADFD;
                    return ptr::null_mut()
                },
            }

            // TODO: parse and use `mode`
            let file = crate::unique_mem_create() as *mut libc::FILE;
            let file_id = FileId::from(file);
            let None = ctx.local().file_objs.insert(file_id, fd) else {
                crate::abort("unexpected duplicate passthrough FILE* object created");
            };

            file
        } else {
            let file = hook_macros::real!(fdopen)(fd, mode);
            if !file.is_null() {
                let file_id = FileId::from(file);
                let None = ctx.local().passthrough_file_objs.insert(file_id, fd) else {
                    crate::abort("unexpected duplicate passthrough FILE* object created");
                };
            }

            file
        }
    }
}

hook_macros::hook! {
    unsafe fn umask(
        mask: libc::mode_t
    ) -> libc::c_int => fizzle_umask(ctx) {
        let res = hook_macros::real!(umask)(mask);

        // TODO: set umask in virtual fs once permissions implemented

        res
    }
}

hook_macros::hook! {
    unsafe fn open(
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_open(ctx) {

        // TODO: file locking is not yet supported here...

        // TODO: track atime

        // TODO: deal with terminals

        // TODO: what about O_TRUNC?

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        // Files are drawn from the underlying filesystem by default.
        // A user may configure certain file paths to be mapped to virtual files.
        // Likewise, files created during the lifetime of fizzle are stored virtually.

        if (flags & libc::O_CREAT) != 0 {
            if (flags & libc::O_EXCL) != 0 && ctx.local().files.contains_key(&path) {
                *libc::__errno_location() = libc::EEXIST;
                return -1
            }

            // TODO: we ignore open mode here
            match ctx.local().files.entry(path.clone()) {
                std::collections::hash_map::Entry::Occupied(_) => (), // TODO: update lock state/timestamp here
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(FileInfo::new());
                },
            }

            let fd = crate::alias_fd_create();
            ctx.local().fds.insert(fd, FdInfo::File(path));
            fd
        } else if (flags & libc::O_PATH) != 0 {
            let fd = hook_macros::real!(open)(pathname, flags, mode);
            ctx.local().fds.insert(fd, FdInfo::Directory(path));
            fd
        } else if !ctx.local().files.contains_key(&path) {
            let fd = hook_macros::real!(open)(pathname, flags, mode);
            if fd == 0 {
                ctx.local().fds.insert(fd, FdInfo::PassthroughFile(path));
            }

            fd
        } else {

            let fd = crate::alias_fd_create();
            ctx.local().fds.insert(fd, FdInfo::File(path));
            fd
        }
    }
}

// By default, files made with creat of O_CREAT will be created in the virtual fs.
hook_macros::hook! {
    unsafe fn creat(
        pathname: *const libc::c_char,
        _mode: libc::mode_t
    ) -> libc::c_int => fizzle_creat(ctx) {

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        // TODO: we ignore open mode here
        match ctx.local().files.entry(path.clone()) {
            std::collections::hash_map::Entry::Occupied(_) => {
                // TODO: this should act as O_TRUNC...
                // TODO: save lock info in entry
                let fd = crate::alias_fd_create();
                ctx.local().fds.insert(fd, FdInfo::File(path));
                fd
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(FileInfo::new());

                let fd = crate::alias_fd_create();
                ctx.local().fds.insert(fd, FdInfo::File(path));
                fd
            },
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

        let Some(fd_info) = ctx.local().fds.get(&dirfd) else {
            *libc::__errno_location() = libc::ENOENT; // TODO: check errno correctness
            return -1
        };

        let FdInfo::Directory(dir_path) = fd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: check errno correctness
            return -1
        };

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = dir_path.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if (flags & libc::O_CREAT) != 0 {
            if (flags & libc::O_EXCL) != 0 && ctx.local().files.contains_key(&path) {
                *libc::__errno_location() = libc::EEXIST;
                return -1
            }

            // TODO: we ignore open mode here
            match ctx.local().files.entry(path.clone()) {
                std::collections::hash_map::Entry::Occupied(_) => (), // TODO: update lock state/timestamp here
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(FileInfo::new());
                },
            }

            let fd = crate::alias_fd_create();
            ctx.local().fds.insert(fd, FdInfo::File(path));
            fd
        } else if (flags & libc::O_PATH) != 0 {
            let fd = hook_macros::real!(open)(pathname, flags, mode);
            ctx.local().fds.insert(fd, FdInfo::Directory(path));
            fd
        } else if !ctx.local().files.contains_key(&path) {
            let fd = hook_macros::real!(openat)(dirfd, pathname, flags, mode);
            if fd == 0 {
                ctx.local().fds.insert(fd, FdInfo::PassthroughFile(path));
            }

            fd
        } else {

            let fd = crate::alias_fd_create();
            ctx.local().fds.insert(fd, FdInfo::File(path));
            fd
        }
    }
}

// TODO: libc::file_handle not defined in `libc` crate
/*
hook_macros::hook! {
    unsafe fn name_to_handle_at(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        handle: *mut libc::file_handle,
        mount_id: *mut libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_name_to_handle_at {

        crate::debug_abort("name_to_handle_at")

        hook_macros::real!(name_to_handle_at)(dirfd, pathname, mount_id, flags)
    }
}

hook_macros::hook! {
    unsafe fn open_by_handle_at(
        mount_fd: libc::c_int,
        pathname: *const libc::c_char,
        mount_id: *mut libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_open_by_handle_at {

        crate::debug_abort("open_by_handle_at")

        hook_macros::real!(name_to_handle_at)(dirfd, pathname, mount_id, flags)
    }
}
*/

hook_macros::hook! {
    unsafe fn fwrite(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwrite(ctx) {

        let file_id = FileId::from(stream);
        match ctx.local().file_objs.get_mut(&file_id) {
            Some(_fd) => if nmemb == 1 { size } else { nmemb }, // TODO: write to emulated file
            None => hook_macros::real!(fwrite)(ptr, size, nmemb, stream),
        }
    }
}

hook_macros::hook! {
    unsafe fn fread(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fread(ctx) {

        let file_id = FileId::from(stream);
        match ctx.local().file_objs.get_mut(&file_id) {
            Some(_fd) => 0, // TODO: read from emulated file
            None => hook_macros::real!(fwrite)(ptr, size, nmemb, stream),
        }
    }
}

hook_macros::hook! {
    unsafe fn fclose(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fclose(ctx) {

        let file_id = FileId::from(stream);
        match ctx.local().file_objs.remove(&file_id) {
            Some(fd) => {
                let Some(_) = ctx.local().fds.remove(&fd) else {
                    crate::abort("invalid internal state (FILE* object with no corresponding FileInfo)")
                };

                0
            },
            None => {
                let ret = hook_macros::real!(fclose)(stream);
                if ret == 0 {
                    let Some(fd) = ctx.local().passthrough_file_objs.remove(&file_id) else {
                        crate::abort("invalid internal state (passthrough FILE* object closed with no entry in fizzle state)")
                    };

                    let Some(_) = ctx.local().fds.remove(&fd) else {
                        crate::abort("invalid internal state (passthrough FILE* object closed with no corresponding FileInfo)")
                    };
                }

                ret
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn chdir(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_chdir(ctx) {

        let res = hook_macros::real!(chdir)(path);

        if res == 0 {
            let Ok(new_abspath) = FilePath::from_cstr(CStr::from_ptr(path)) else {
                *libc::__errno_location() = libc::EINVAL;
                return -1
            };

            ctx.local().working_directory = new_abspath;
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn fchdir(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_fchdir(ctx) {

        let res = hook_macros::real!(fchdir)(fd);
        if res == 0 {
            if let Some(FdInfo::File(path) | FdInfo::PassthroughFile(path)) = ctx.local().fds.get(&fd) {
                ctx.local().working_directory = path.clone();
            }else {
                crate::abort("`fchdir` called on unrecognized fd");
            }
        }

        res
    }
}

hook_macros::hook! {
    unsafe fn chroot(
        _path: *const libc::c_char
    ) -> libc::c_int => fizzle_chroot(ctx) {

        crate::abort("`chroot` not implemented");
    }
}

// Don't likely need to handle in any meaningful way (other than checking file existence):

hook_macros::hook! {
    unsafe fn chown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_chown(ctx) {

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
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

        if ctx.local().fds.contains_key(&fd) {
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

        if ctx.local().fds.contains_key(&fd) {
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(fd_info) = ctx.local().fds.get(&dirfd) else {
            crate::abort("unrecognized dirfd passed to `fchownat`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_path) = fd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(path) = dir_path.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(fd_info) = ctx.local().fds.get(&dirfd) else {
            crate::abort("unrecognized dirfd passed to `fchmodat`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_path) = fd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(path) = dir_path.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
            0 // TODO: handle ownership permissions?
        } else {
            hook_macros::real!(lchown)(pathname, owner, group)
        }
    }
}

hook_macros::hook! {
    unsafe fn fseek(
        stream: *mut libc::FILE,
        offset: libc::c_long,
        whence: libc::c_int
    ) -> libc::c_int => fizzle_fseek(ctx) {

        let file_id = FileId::from(stream);
        if let Some(_) = ctx.local().file_objs.get(&file_id) {
            0 // TODO: handle passthrough
        } else {
            hook_macros::real!(fseek)(stream, offset, whence)
        }
    }
}

hook_macros::hook! {
    unsafe fn ftell(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_ftell(ctx) {

        let file_id = FileId::from(stream);
        if let Some(_) = ctx.local().file_objs.get(&file_id) {
            0 // TODO: handle passthrough
        } else {
            hook_macros::real!(ftell)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn frewind(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_frewind(ctx) {

        let file_id = FileId::from(stream);
        if let Some(_) = ctx.local().file_objs.get(&file_id) {
            0 // TODO: handle passthrough
        } else {
            hook_macros::real!(frewind)(stream)
        }
    }
}

hook_macros::hook! {
    unsafe fn fgetpos(
        stream: *mut libc::FILE,
        pos: *mut libc::fpos_t
    ) -> libc::c_int => fizzle_fgetpos(ctx) {

        let file_id = FileId::from(stream);
        if let Some(_) = ctx.local().file_objs.get(&file_id) {
            0 // TODO: handle passthrough
        } else {
            hook_macros::real!(fgetpos)(stream, pos)
        }
    }
}

hook_macros::hook! {
    unsafe fn fsetpos(
        stream: *mut libc::FILE,
        pos: *const libc::fpos_t
    ) -> libc::c_int => fizzle_fsetpos(ctx) {

        let file_id = FileId::from(stream);
        if let Some(_) = ctx.local().file_objs.get(&file_id) {
            0 // TODO: handle passthrough
        } else {
            hook_macros::real!(fsetpos)(stream, pos)
        }
    }
}

hook_macros::hook! {
    unsafe fn access(
        pathname: *mut libc::c_char,
        mode: libc::c_int
    ) -> libc::c_int => fizzle_access(ctx) {

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if let Some(_) = ctx.local().files.get(&path) {
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(fd_info) = ctx.local().fds.get(&dirfd) else {
            crate::abort("unrecognized dirfd passed to `faccessat`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_path) = fd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(path) = dir_path.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if let Some(_) = ctx.local().files.get(&path) {
            crate::abort("function `stat` unimplimented for fizzle virtual fs")
            // TODO: implement
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(path) = ctx.local().working_directory.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if let Some(_) = ctx.local().files.get(&path) {
            crate::abort("function `stat` unimplimented for fizzle virtual fs")
            // TODO: implement
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

        if ctx.local().fds.contains_key(&fd) {
            crate::abort("function `fstat` unimplemented for fizzle virtual fs")
            // TODO: implement
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

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(fd_info) = ctx.local().fds.get(&dirfd) else {
            crate::abort("unrecognized dirfd passed to `fstatat`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_path) = fd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(path) = dir_path.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
            crate::abort("function `fstatat` unimplemented for fizzle virtual fs")
            // TODO: implement
        } else {
            hook_macros::real!(fstatat)(dirfd, pathname, statbuf, flags)
        }
    }
}

hook_macros::hook! {
    unsafe fn statx(
        dirfd: libc::c_int,
        pathname: *mut libc::c_char,
        flags: libc::c_int,
        mask: libc::c_uint,
        statxbuf: *mut libc::statx
    ) -> libc::c_int => fizzle_statx(ctx) {

        let Ok(relative_path) = FilePath::from_cstr(CStr::from_ptr(pathname)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(fd_info) = ctx.local().fds.get(&dirfd) else {
            crate::abort("unrecognized dirfd passed to `statx`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_path) = fd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(path) = dir_path.clone().concat(&relative_path) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if ctx.local().files.contains_key(&path) {
            crate::abort("function `statx` unimplemented for fizzle virtual fs")
            // TODO: implement
        } else {
            hook_macros::real!(statx)(dirfd, pathname, flags, mask, statxbuf)
        }
    }
}

hook_macros::hook! {
    unsafe fn readlink(
        pathname: *mut libc::c_char,
        buf: *mut libc::c_char,
        bufsiz: libc::size_t
    ) -> libc::c_int => fizzle_readlink(ctx) {
        hook_macros::real!(readlink)(pathname, buf, bufsiz)
    }
}

hook_macros::hook! {
    unsafe fn readlinkat(
        dirfd: libc::c_int,
        pathname: *mut libc::c_char,
        buf: *mut libc::c_char,
        bufsiz: libc::size_t
    ) -> libc::c_int => fizzle_readlinkat(ctx) {
        hook_macros::real!(readlinkat)(dirfd, pathname, buf, bufsiz)
    }
}

hook_macros::hook! {
    unsafe fn symlink(
        target: *mut libc::c_char,
        linkpath: *const libc::c_char
    ) -> libc::c_int => fizzle_symlink(ctx) {
        hook_macros::real!(symlink)(target, linkpath)
    }
}

hook_macros::hook! {
    unsafe fn symlinkat(
        target: *mut libc::c_char,
        newdirfd: libc::c_int,
        linkpath: *const libc::c_char
    ) -> libc::c_int => fizzle_symlinkat(ctx) {
        hook_macros::real!(symlinkat)(target, newdirfd, linkpath)
    }
}

hook_macros::hook! {
    unsafe fn link(
        oldpath: *mut libc::c_char,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_link(ctx) {
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
    ) -> libc::c_int => fizzle_linkat(ctx) {
        hook_macros::real!(linkat)(olddirfd, oldpath, newdirfd, newpath, flags)
    }
}

hook_macros::hook! {
    unsafe fn unlink(
        pathname: *const libc::c_char
    ) -> libc::c_int => fizzle_unlink(ctx) {
        hook_macros::real!(unlink)(pathname)
    }
}

hook_macros::hook! {
    unsafe fn unlinkat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_unlinkat(ctx) {
        hook_macros::real!(unlinkat)(dirfd, pathname, flags)
    }
}

hook_macros::hook! {
    unsafe fn rename(
        oldpath: *mut libc::c_char,
        newpath: *const libc::c_char
    ) -> libc::c_int => fizzle_rename(ctx) {

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(abs_oldpath) = ctx.local().working_directory.clone().concat(&rel_oldpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(_abs_newpath) = ctx.local().working_directory.clone().concat(&rel_newpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if let Some(_) = ctx.local().files.remove(&abs_oldpath) {
            crate::abort("function `rename` not implemented for fizzle virtual fs");
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

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(oldfd_info) = ctx.local().fds.get(&olddirfd) else {
            crate::abort("unrecognized olddirfd passed to `renameat`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_oldpath) = oldfd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(abs_oldpath) = dir_oldpath.clone().concat(&rel_oldpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(newfd_info) = ctx.local().fds.get(&newdirfd) else {
            crate::abort("unrecognized newdirfd passed to `renameat`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_newpath) = newfd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(_abs_newpath) = dir_newpath.clone().concat(&rel_newpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if let Some(_) = ctx.local().files.remove(&abs_oldpath) {
            crate::abort("function `renameat` not implemented for fizzle virtual fs");
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

        let Ok(rel_oldpath) = FilePath::from_cstr(CStr::from_ptr(oldpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(oldfd_info) = ctx.local().fds.get(&olddirfd) else {
            crate::abort("unrecognized olddirfd passed to `renameat2`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_oldpath) = oldfd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(abs_oldpath) = dir_oldpath.clone().concat(&rel_oldpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Some(newfd_info) = ctx.local().fds.get(&newdirfd) else {
            crate::abort("unrecognized newdirfd passed to `renameat2`");
            // TODO: downgrade this to a warning in the future and return the following
            // *libc::__errno_location() = libc::ENOENT;
            // return -1
        };

        let FdInfo::Directory(dir_newpath) = newfd_info else {
            *libc::__errno_location() = libc::EBADFD; // TODO: verify correct err code
            return -1
        };

        let Ok(rel_newpath) = FilePath::from_cstr(CStr::from_ptr(newpath)) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        let Ok(_abs_newpath) = dir_newpath.clone().concat(&rel_newpath) else {
            *libc::__errno_location() = libc::EINVAL;
            return -1
        };

        if let Some(_) = ctx.local().files.remove(&abs_oldpath) {
            crate::abort("function `renameat2` not implemented for fizzle virtual fs");
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
    ) -> libc::c_int => fizzle_mknod(ctx) {
        hook_macros::real!(mknod)(pathname, mode, dev)
    }
}

hook_macros::hook! {
    unsafe fn mknodat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        mode: libc::mode_t,
        dev: libc::dev_t
    ) -> libc::c_int => fizzle_mknodat(ctx) {
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
    ) -> libc::c_int => fizzle_mount(ctx) {
        hook_macros::real!(mount)(source, target, filesystemtype, mountflags, data)
    }
}

hook_macros::hook! {
    unsafe fn umount(
        target: *const libc::c_char
    ) -> libc::c_int => fizzle_umount(ctx) {
        hook_macros::real!(umount)(target)
    }
}

hook_macros::hook! {
    unsafe fn umount2(
        target: *const libc::c_char,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_umount2(ctx) {
        hook_macros::real!(umount2)(target, flags)
    }
}
