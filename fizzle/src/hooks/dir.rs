use crate::hook_macros;
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

hook_macros::hook! {
    unsafe fn opendir(
        name: *const libc::c_char
    ) -> *mut libc::DIR => fizzle_opendir(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function opendir() called within signal handler")
            }
        }

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::opendir(name) };
        #[cfg(not(feature = "passthroughfs"))]
        return unsafe { libc::opendir(name) };
    }
}

hook_macros::hook! {
    unsafe fn fdopendir(
        fd: libc::c_int
    ) -> *mut libc::DIR => fizzle_fdopendir(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function fdopendir() called within signal handler")
            }
        }

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::fdopendir(fd) };
        #[cfg(not(feature = "passthroughfs"))]
        return unsafe { libc::fdopendir(fd) };
    }
}

hook_macros::hook! {
    unsafe fn dirfd(
        dirp: *mut libc::DIR
    ) -> libc::c_int => fizzle_dirfd(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::dirfd(dirp) };
        #[cfg(not(feature = "passthroughfs"))]
        return unsafe { libc::dirfd(dirp) };
    }
}

hook_macros::hook! {
    unsafe fn closedir(
        dirp: *mut libc::DIR
    ) -> libc::c_int => fizzle_closedir(_ctx) {
        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function closedir() called within signal handler")
            }
        }
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::closedir(dirp) };
        #[cfg(not(feature = "passthroughfs"))]
        return unsafe { libc::closedir(dirp) };
    }
}

hook_macros::hook! {
    unsafe fn readdir(
        _dirp: *mut libc::DIR
    ) -> *mut libc::dirent => fizzle_readdir(_ctx) {

        #[cfg(feature = "sigsan")] {
            if in_sighandler() {
                panic!("async-signal-unsafe function readdir() called within signal handler")
            }
        }

        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::readdir(_dirp) };
        #[cfg(not(feature = "passthroughfs"))]
        unimplemented!("readdir()")
    }
}

/*
type FtwFn = unsafe extern "C" fn(fpath: *const char, sb: *const libc::stat, typeflag: libc::c_int);

hook_macros::hook! {
    unsafe fn ftw(
        dirpath: *const libc::c_char,
        ftw_fn: FtwFn,
        nopenfd: libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_ftw(_ctx) {
        #[cfg(feature = "passthroughfs")]
        return unsafe { libc::ftw(dirpath, ftw_fn, nopenfd, flags) };

        unimplemented!("ftw()")
    }
}
*/

/*
type NftwFn = unsafe extern "C" fn(
    fpath: *const char,
    sb: *const libc::stat,
    typeflag: libc::c_int,
    ftwbuf: *mut libc::c_void,
);

hook_macros::hook! {
    unsafe fn nftw(
        _dirpath: *const libc::c_char,
        _fn: NftwFn,
        _nopenfd: libc::c_int,
        _flags: libc::c_int
    ) -> libc::c_int => fizzle_nftw(_ctx) {
        unimplemented!("nftw()")
    }
}
*/

/*
type FTS = libc::c_void;
type FTSENT = libc::c_void;
type FtsCompareFn = unsafe extern "C" fn(*const *const FTSENT, *const *const FTSENT);

hook_macros::hook! {
    unsafe fn fts_open(
        _path_argv: *const *const libc::c_char,
        _options: libc::c_int,
        _compar: FtsCompareFn
    ) -> *mut FTS => fizzle_fts_open(_ctx) {
        unimplemented!("fts_open()")
    }
}

hook_macros::hook! {
    unsafe fn fts_read(
        _ftsp: *mut FTS
    ) -> *mut FTSENT => fizzle_fts_read(_ctx) {
        unimplemented!("fts_read()")
    }
}

hook_macros::hook! {
    unsafe fn fts_children(
        _ftsp: *mut FTS,
        _instr: libc::c_int
    ) -> *mut FTSENT => fizzle_fts_children(_ctx) {
        unimplemented!("fts_children()")
    }
}

hook_macros::hook! {
    unsafe fn fts_set(
        _ftsp: *mut FTS,
        _f: *mut FTSENT,
        _instr: libc::c_int
    ) -> libc::c_int => fizzle_fts_set(_ctx) {
        unimplemented!("fts_set()")
    }
}

hook_macros::hook! {
    unsafe fn fts_close(
        _ftsp: *mut FTS
    ) -> libc::c_int => fizzle_fts_close(_ctx) {
        unimplemented!("fts_close()")
    }
}

*/
