use std::ffi::VaList;

unsafe extern "C" {
    #[cfg(feature = "afl")]
    pub fn __afl_manual_init();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_on();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_off();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_discard();

    #[cfg(feature = "afl")]
    pub fn __afl_coverage_skip();

    #[cfg(feature = "afl")]
    pub fn __afl_auto_early();

    #[cfg(feature = "afl")]
    pub fn __afl_auto_first();

    #[cfg(feature = "afl")]
    pub fn __afl_auto_second();

    #[cfg(feature = "pcr")]
    pub static __afl_already_initialized_second: u32;

    // TODO: three underscores for Apple
    #[cfg(feature = "pcr")]
    pub fn __afl_persistent_loop(input: libc::c_uint) -> libc::c_int;

    #[cfg(feature = "pcr")]
    pub static __afl_fuzz_len: *mut libc::c_uint;

    #[cfg(feature = "pcr")]
    pub static __afl_fuzz_ptr: *mut libc::c_uchar;

    #[cfg(feature = "pcr")]
    pub static __afl_connected: libc::c_int;

    #[cfg(feature = "pcr")]
    pub static mut __afl_sharedmem_fuzzing: libc::c_int;

    pub fn vasprintf(
        strp: *mut *mut libc::c_char,
        fmt: *const libc::c_char,
        ap: VaList,
    ) -> libc::c_int;

    pub fn vsscanf(
        str: *const libc::c_char,
        format: *const libc::c_char,
        ap: VaList
    ) -> libc::c_int;

    pub fn res_mkquery(
        op: libc::c_int,
        dname: *const libc::c_char,
        class: libc::c_int,
        ty: libc::c_int,
        data: *const libc::c_uchar,
        datalen: libc::c_int,
        newrr: *const libc::c_uchar,
        buf: *mut libc::c_uchar,
        buflen: libc::c_int
    ) -> libc::c_int;

    pub fn pthread_attr_getdetachstate(
        attr: *const libc::pthread_attr_t,
        detachstate: *mut libc::c_int,
    ) -> libc::c_int;

    pub fn pthread_mutexattr_gettype(
        attr: *const libc::pthread_mutexattr_t,
        kind: *mut libc::c_int,
    ) -> libc::c_int;

    pub static mut stdin: *mut libc::FILE;

    pub static mut stdout: *mut libc::FILE;

    pub static mut stderr: *mut libc::FILE;
}
