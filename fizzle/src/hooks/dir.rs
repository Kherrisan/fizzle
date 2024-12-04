use crate::hook_macros;

hook_macros::hook! {
    unsafe fn opendir(
        _name: *const libc::c_char
    ) -> *mut libc::DIR => fizzle_opendir(_ctx) {
        unimplemented!("opendir()")
    }
}

hook_macros::hook! {
    unsafe fn fdopendir(
        _fd: libc::c_int
    ) -> *mut libc::DIR => fizzle_fdopendir(_ctx) {
        unimplemented!("fdopendir()")
    }
}

hook_macros::hook! {
    unsafe fn dirfd(
        _dirp: *mut libc::DIR
    ) => fizzle_dirfd(_ctx) {
        unimplemented!("dirfd()")
    }
}

hook_macros::hook! {
    unsafe fn closedir(
        _name: *const libc::c_char
    ) -> *mut libc::DIR => fizzle_closedir(_ctx) {
        unimplemented!("closedir()")
    }
}

hook_macros::hook! {
    unsafe fn readdir(
        _dirp: *mut libc::DIR
    ) -> *mut libc::dirent => fizzle_readdir(_ctx) {
        unimplemented!("readdir()")
    }
}

type FtwFn = unsafe extern "C" fn(fpath: *const char, sb: *const libc::stat, typeflag: libc::c_int);

hook_macros::hook! {
    unsafe fn ftw(
        _dirpath: *const libc::c_char,
        _fn: FtwFn,
        _nopenfd: libc::c_int,
        _flags: libc::c_int
    ) -> libc::c_int => fizzle_ftw(_ctx) {
        unimplemented!("ftw()")
    }
}

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
