use crate::{hook_macros, state};

hook_macros::hook! {
    unsafe fn fdopen(
        fd: libc::c_int,
        mode: *const libc::c_char
    ) -> *mut libc::FILE => fizzle_fdopen {
        let file = hook_macros::real!(fdopen)(fd, mode);

        file
    }
}

hook_macros::hook! {
    unsafe fn open(
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t
    ) -> libc::c_int => fizzle_open {

        let mut state = state::fizzle_state().lock().unwrap();
        
        

        crate::debug_abort("open");

        let fd = hook_macros::real!(open)(pathname, flags, mode);

        fd
    }
}

hook_macros::hook! {
    unsafe fn fwrite(
        ptr: *mut libc::c_void,
        size: libc::size_t,
        nmemb: libc::size_t,
        stream: *mut libc::FILE
    ) -> libc::size_t => fizzle_fwrite {

        crate::debug_abort("fwrite");

        hook_macros::real!(fwrite)(ptr, size, nmemb, stream)
    }
}

hook_macros::hook! {
    unsafe fn fclose(
        stream: *mut libc::FILE
    ) -> libc::c_int => fizzle_fclose {

        crate::debug_abort("fclose");

        hook_macros::real!(fclose)(stream)
    }
}

hook_macros::hook! {
    unsafe fn chdir(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_chdir {

        crate::debug_abort("chdir");

        hook_macros::real!(chdir)(path)
    }
}

hook_macros::hook! {
    unsafe fn fchdir(
        fd: libc::c_int
    ) -> libc::c_int => fizzle_fchdir {

        crate::debug_abort("fchdir");

        hook_macros::real!(fchdir)(fd)
    }
}

hook_macros::hook! {
    unsafe fn chroot(
        path: *const libc::c_char
    ) -> libc::c_int => fizzle_chroot {

        crate::debug_abort("chroot");

        hook_macros::real!(chroot)(path)
    }
}

// Don't likely need to handle in any meaningful way (other than checking file existence):

hook_macros::hook! {
    unsafe fn chown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_chown {

        crate::debug_abort("chown");

        hook_macros::real!(chown)(pathname, owner, group)
    }
}

hook_macros::hook! {
    unsafe fn fchown(
        fd: libc::c_int,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_fchown {

        crate::debug_abort("fchown");

        hook_macros::real!(fchown)(fd, owner, group)
    }
}

hook_macros::hook! {
    unsafe fn lchown(
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t
    ) -> libc::c_int => fizzle_lchown {

        crate::debug_abort("lchown");

        hook_macros::real!(lchown)(pathname, owner, group)
    }
}

hook_macros::hook! {
    unsafe fn fchownat(
        dirfd: libc::c_int,
        pathname: *const libc::c_char,
        owner: libc::uid_t,
        group: libc::gid_t,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_fchownat {

        crate::debug_abort("fchownat");

        hook_macros::real!(fchownat)(dirfd, pathname, owner, group, flags)
    }
}
