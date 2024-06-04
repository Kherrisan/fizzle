//! Process creation shims.
//!
//!

use std::ffi::{CStr, CString};
use std::{array, mem, ptr, thread};

use crate::constants::FIZZLE_MEMORY_ENV;
use crate::hook_macros;
use crate::state::identifiers::WorkerId;

const MAX_ARGS: usize = 512;

hook_macros::hook! {
    unsafe fn fork() -> libc::pid_t => fizzle_fork(ctx) {

        let pid = hook_macros::real!(fork)();
        match pid {
            0 => {
                // Child process--fix all of the local state
                ctx.local().plugin_modules = None;

                // TODO: upref all reference-counted global variables here
                // For now we just don't free global variables so it's fine...
            }
            1.. => {
                // Parent process--await execution

                let thread_id = thread::current().id();

                // This thread should still be able to execute afterwards
                ctx.add_ready_thread(thread_id);

                // This process should still be able to execute afterwards
                let process_id = ctx.local().process_id();

                ctx.global().mark_worker_ready(WorkerId {
                    process_id: process_id,
                    thread_id: thread_id,
                });

                // Pause our process until it gets delegated execution again.
                ctx.pause_current_process();
            }
            _ => () // else fork() returned -1 and failed--do nothing
        }

        pid
    }
}

hook_macros::va_args_hook! {
    unsafe extern "C" fn execl(pathname: *const libc::c_char, arg: *const libc::c_char) -> libc::c_int => preload(ctx, va_args) {
        let mut end_reached = false;
        let argv: [*const libc::c_char; MAX_ARGS] = array::from_fn(|i| {
            if end_reached {
                ptr::null()
            } else {
                let arg: *const libc::c_char = va_args.arg();
                if arg.is_null() {
                    end_reached = true;
                }
                arg
            }
        });

        if !end_reached {
            panic!("`execl` exceeded maximum number of va_args")
        }

        drop(ctx);
        fizzle_execv(pathname, ptr::addr_of!(argv) as *const *const libc::c_char)
    }
}

hook_macros::va_args_hook! {
    unsafe extern "C" fn execlp(file: *const libc::c_char, arg: *const libc::c_char) -> libc::c_int => preload(ctx, va_args) {
        let mut end_reached = false;
        let argv: [*const libc::c_char; MAX_ARGS] = array::from_fn(|_| {
            if end_reached {
                ptr::null()
            } else {
                let arg: *const libc::c_char = va_args.arg();
                if arg.is_null() {
                    end_reached = true;
                }
                arg
            }
        });

        if !end_reached {
            panic!("`execl` exceeded maximum number of va_args")
        }

        drop(ctx);
        fizzle_execvp(file, ptr::addr_of!(argv) as *const *const libc::c_char)
    }
}

hook_macros::va_args_hook! {
    unsafe extern "C" fn execle(pathname: *const libc::c_char, arg: *const libc::c_char) -> libc::c_int => preload(ctx, va_args) {
        let mut envp: Option<*const *const libc::c_char> = None;
        let argv: [*const libc::c_char; MAX_ARGS] = array::from_fn(|_| {
            if envp.is_some() {
                ptr::null()
            } else {
                let arg: *const libc::c_char = va_args.arg();
                if arg.is_null() {
                    envp = Some(va_args.arg());
                }
                arg
            }
        });


        let Some(envp) = envp else {
            panic!("`execle` exceeded maximum number of va_args")
        };

        drop(ctx);
        fizzle_execve(pathname, ptr::addr_of!(argv) as *const *const libc::c_char, envp)
    }
}

hook_macros::hook! {
    unsafe fn execv(pathname: *const libc::c_char, argv: *const *const libc::c_char) -> libc::c_int => fizzle_execv(ctx) {
        // env is inherited, so no variables need to be defined
        assert!(ctx.local().plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)
        hook_macros::real!(execv)(pathname, argv)
    }
}

hook_macros::hook! {
     unsafe fn execvp(file: *const libc::c_char, argv: *const *const libc::c_char) -> libc::c_int => fizzle_execvp(ctx) {
        // env is inherited, so no variables need to be defined
        assert!(ctx.local().plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)
        hook_macros::real!(execvp)(file, argv)
    }
}

hook_macros::hook! {
     unsafe fn execve(pathname: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_execve(ctx) {
        let mut envp_idx = 0;

        assert!(ctx.local().plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)

        // TODO: make this less messy
        let fizzle_env = CString::new(format!("{}={}", FIZZLE_MEMORY_ENV.to_str().unwrap(), CStr::from_ptr(libc::getenv(FIZZLE_MEMORY_ENV.as_ptr())).to_str().unwrap())).unwrap();
        let env: [*const libc::c_char; MAX_ARGS] = array::from_fn(|i| {
            if i != envp_idx {
                return ptr::null()
            }

            let e = unsafe { *envp.add(envp_idx) };
            if e.is_null() {
                fizzle_env.as_ptr()
            } else {
                envp_idx += 1;
                e
            }
        });

        // Ensures that `fizzle_env` remains valid at least until `execve` is called
        mem::forget(fizzle_env);

        if envp_idx == MAX_ARGS {
            panic!("`execve` exceeded maximum number of env variables")
        }

        hook_macros::real!(execve)(pathname, argv, ptr::addr_of!(env) as *const *const libc::c_char)
    }
}

hook_macros::hook! {
    unsafe fn execveat(dirfd: libc::c_int, pathname: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char, flags: libc::c_int) -> libc::c_int => fizzle_execveat(ctx) {
        crate::report_strict_failure("unimplemented `execveat`");
        hook_macros::real!(execveat)(dirfd, pathname, argv, envp, flags)
    }
}

hook_macros::hook! {
    unsafe fn fexecve(fd: libc::c_int, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_fexecve(ctx) {
        crate::report_strict_failure("unimplemented `fexecve`");
        hook_macros::real!(fexecve)(fd, argv, envp)
    }
}

hook_macros::hook! {
     unsafe fn execvpe(file: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_execvpe(ctx) {
        let mut envp_idx = 0;

        assert!(ctx.local().plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)

        // TODO: make this less messy
        let fizzle_env = CString::new(format!("{}={}", FIZZLE_MEMORY_ENV.to_str().unwrap(), CStr::from_ptr(libc::getenv(FIZZLE_MEMORY_ENV.as_ptr())).to_str().unwrap())).unwrap();
        let env: [*const libc::c_char; MAX_ARGS] = array::from_fn(|i| {
            if i != envp_idx {
                return ptr::null()
            }

            let e = unsafe { *envp.add(envp_idx) };
            if e.is_null() {
                fizzle_env.as_ptr()
            } else {
                envp_idx += 1;
                e
            }
        });

        // Ensures that `fizzle_env` remains valid at least until `execve` is called
        mem::forget(fizzle_env);

        if envp_idx == MAX_ARGS {
            panic!("`execve` exceeded maximum number of env variables")
        }

        hook_macros::real!(execvpe)(file, argv, ptr::addr_of!(env) as *const *const libc::c_char)
    }
}

hook_macros::hook! {
     unsafe fn system(command: *const libc::c_char) -> libc::c_int => fizzle_system(_ctx) {
        // env is inherited, so no variables need to be defined
        let fizzle_memory = CString::from_raw(libc::getenv(FIZZLE_MEMORY_ENV.as_ptr()));
        libc::unsetenv(FIZZLE_MEMORY_ENV.as_ptr());
        let res = hook_macros::real!(system)(command); // `system` commands are executed without any Fizzle harness
        libc::setenv(FIZZLE_MEMORY_ENV.as_ptr(), fizzle_memory.as_ptr(), 1);
        res
    }
}

hook_macros::hook! {
    unsafe fn exit(status: libc::c_int) => fizzle_exit(ctx) {
        if ctx.local().suspend_on_exit {
            // TODO: clean up any polling contexts here so that this process never gets
            // delegated to (other than for the purpose of running modules)

            // Temporary hack: whenever processes get delegated to here, just pass back to
            // another process (i.e. ignore inputs)
            loop {
                ctx.yield_thread()
            }
        } else {
            hook_macros::real!(exit)(status)
        }
    }
}

hook_macros::hook! {
    unsafe fn _exit(status: libc::c_int) => fizzle_exit2(ctx) {
        if ctx.local().suspend_on_exit {
            // TODO: clean up any polling contexts here so that this process never gets
            // delegated to (other than for the purpose of running modules)

            // Temporary hack: whenever processes get delegated to here, just pass back to
            // another process (i.e. ignore inputs)
            loop {
                ctx.yield_thread()
            }
        } else {
            hook_macros::real!(exit)(status)
        }
    }
}

// We need this to ensure that our `atexit` hook is called first when FIZZLE_NOEXIT is set.
hook_macros::hook! {
    unsafe fn atexit(cb: extern "C" fn()) => fizzle_atexit(_ctx) {
        hook_macros::real!(atexit)(cb)
    }
}
