use crate::hook_macros;

//libc::syscall(1,2,3,4);


hook_macros::va_args_hook! {
    unsafe extern "C" fn syscall(
        flags: libc::c_int
    ) -> libc::c_int => fizzle_syscall(ctx, va_args) {
        panic!("raw `syscall` unsupported by Fizzle");
    }
}