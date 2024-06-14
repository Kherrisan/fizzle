//! Process creation shims.
//!
//!

use std::ffi::CString;
use std::{array, env, mem, ptr, thread};

use crate::constants::FIZZLE_MEMORY_ENV;
use crate::hook_macros;
use crate::state;

const MAX_ARGS: usize = 512;

hook_macros::hook! {
    unsafe fn fork() -> libc::pid_t => fizzle_fork(ctx) {
        let thread_id = thread::current().id();

        // This thread should still be able to execute afterwards
        ctx.mark_thread_ready(thread_id);

        let pid = hook_macros::real!(fork)();
        match pid {
            0 => {
                // Child process--fix all of the local state
                ctx.local.plugin_modules = None;

                // Assign a new process ID
                let process_id = ctx.global.assign_process_id();
                ctx.local.process_id = process_id;

                // TODO: upref all reference-counted global variables here
                // For now we just don't free global variables so it's fine...
            }
            1.. => {
                // Parent process--await execution
                drop(ctx);
                state::FIZZLE_STATE.pause_current_process();
            }
            _ => {
                // fork() returned -1, but we marked our own process as ready so we need to wait
                drop(ctx);
                state::FIZZLE_STATE.yield_thread();
            }
        }

        pid
    }
}

#[no_mangle]
pub unsafe extern "C" fn execl(pathname: *const libc::c_char, arg: *const libc::c_char, mut va_args: ...) -> libc::c_int {
        
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `syscall` not allowed");
    }
    crate::state::set_entered_handler(true);

    log::trace!(
        "Thread {:?} invoked function `syscall`",
        std::thread::current().id(),
    );

    let mut end_reached = false;
    let argv: [*const libc::c_char; MAX_ARGS] = array::from_fn(|i| {
        if i == 0 {
            arg
        } else if end_reached {
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

    let ret = fizzle_execv(pathname, ptr::addr_of!(argv) as *const *const libc::c_char);

    log::trace!(
        "Function `execl` returned {:?}", // TODO: add process info in the future
        ret
    );
    crate::state::set_entered_handler(false);

    ret
}



#[no_mangle]
pub unsafe extern "C" fn execlp(pathname: *const libc::c_char, arg: *const libc::c_char, mut va_args: ...) -> libc::c_int {
        
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `syscall` not allowed");
    }
    crate::state::set_entered_handler(true);

    log::trace!(
        "Thread {:?} invoked function `syscall`",
        std::thread::current().id(),
    );

    let mut end_reached = false;
    let argv: [*const libc::c_char; MAX_ARGS] = array::from_fn(|i| {
        if i == 0 {
            arg
        } else if end_reached {
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

    let ret = fizzle_execvp(pathname, ptr::addr_of!(argv) as *const *const libc::c_char);

    log::trace!(
        "Function `execl` returned {:?}", // TODO: add process info in the future
        ret
    );
    crate::state::set_entered_handler(false);

    ret
}

#[no_mangle]
pub unsafe extern "C" fn execle(pathname: *const libc::c_char, mut arg: *const libc::c_char, mut va_args: ...) -> libc::c_int {
        
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `syscall` not allowed");
    }
    crate::state::set_entered_handler(true);

    log::trace!(
        "Thread {:?} invoked function `syscall`",
        std::thread::current().id(),
    );

    let mut envp: Option<*const *const libc::c_char> = None;
    let argv: [*const libc::c_char; MAX_ARGS] = array::from_fn(|i| {
        if envp.is_some() {
            ptr::null()
        } else {
            if i != 0 {
                arg = va_args.arg()
            }
            
            if arg.is_null() {
                envp = Some(va_args.arg());
            }
            arg
        }
    });

    let Some(envp) = envp else {
        panic!("`execle` exceeded maximum number of va_args")
    };

    let ret = fizzle_execve(pathname, ptr::addr_of!(argv) as *const *const libc::c_char, envp);

    log::trace!(
        "Function `execl` returned {:?}", // TODO: add process info in the future
        ret
    );
    crate::state::set_entered_handler(false);

    ret
}

hook_macros::hook! {
    unsafe fn execv(pathname: *const libc::c_char, argv: *const *const libc::c_char) -> libc::c_int => fizzle_execv(ctx) {
        // env is inherited, so no variables need to be defined
        assert!(ctx.local.plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)

        // Ensure process ID gets passed through correctly
        let process_id = ctx.local.process_id;
        ctx.global.passthrough_process_id = process_id;
        ctx.copy_exec_fds();
        hook_macros::real!(execv)(pathname, argv)
    }
}

hook_macros::hook! {
     unsafe fn execvp(file: *const libc::c_char, argv: *const *const libc::c_char) -> libc::c_int => fizzle_execvp(ctx) {
        // env is inherited, so no variables need to be defined
        assert!(ctx.local.plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)
        let process_id = ctx.local.process_id;
        ctx.global.passthrough_process_id = process_id;
        ctx.copy_exec_fds();
        hook_macros::real!(execvp)(file, argv)
    }
}

hook_macros::hook! {
     unsafe fn execve(pathname: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_execve(ctx) {
        let mut envp_idx = 0;

        assert!(ctx.local.plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)

        let fizzle_env = CString::new(format!("{}={}", FIZZLE_MEMORY_ENV, env::var(FIZZLE_MEMORY_ENV).unwrap())).unwrap();

        let mut env: [*const libc::c_char; MAX_ARGS] = array::from_fn(|_| {
            let e = unsafe { *envp.add(envp_idx) };
            if e.is_null() {
                ptr::null()
            } else {
                envp_idx += 1;
                e
            }
        });

        // Add our fizzle env to the end of this list
        env[envp_idx] = fizzle_env.as_ptr();
        // Ensures that `fizzle_env` remains valid at least until `execve` is called
        mem::forget(fizzle_env);

        assert!(envp_idx + 1 < MAX_ARGS, "`execve` exceeded maximum number of env variables");

        let process_id = ctx.local.process_id;
        ctx.global.passthrough_process_id = process_id;
        ctx.copy_exec_fds();
        hook_macros::real!(execve)(pathname, argv, ptr::addr_of!(env) as *const *const libc::c_char)
    }
}

hook_macros::hook! {
    unsafe fn execveat(dirfd: libc::c_int, pathname: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char, flags: libc::c_int) -> libc::c_int => fizzle_execveat(ctx) {
        crate::report_strict_failure("unimplemented `execveat`");
        let process_id = ctx.local.process_id;
        ctx.global.passthrough_process_id = process_id;
        ctx.copy_exec_fds();
        hook_macros::real!(execveat)(dirfd, pathname, argv, envp, flags)
    }
}

hook_macros::hook! {
    unsafe fn fexecve(fd: libc::c_int, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_fexecve(ctx) {
        crate::report_strict_failure("unimplemented `fexecve`");
        let process_id = ctx.local.process_id;
        ctx.global.passthrough_process_id = process_id;
        ctx.copy_exec_fds();
        hook_macros::real!(fexecve)(fd, argv, envp)
    }
}

hook_macros::hook! {
     unsafe fn execvpe(file: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_execvpe(ctx) {
        let mut envp_idx = 0;

        assert!(ctx.local.plugin_modules.is_none()); // TODO: handle this edge case (parent is `exec`d)

        let fizzle_env = CString::new(format!("{}={}", FIZZLE_MEMORY_ENV, env::var(FIZZLE_MEMORY_ENV).unwrap())).unwrap();

        let mut env: [*const libc::c_char; MAX_ARGS] = array::from_fn(|_| {
            let e = unsafe { *envp.add(envp_idx) };
            if e.is_null() {
                ptr::null()
            } else {
                envp_idx += 1;
                e
            }
        });

        // Add our fizzle env to the end of this list
        env[envp_idx] = fizzle_env.as_ptr();
        // Ensures that `fizzle_env` remains valid at least until `execve` is called
        mem::forget(fizzle_env);

        if envp_idx == MAX_ARGS {
            panic!("`execve` exceeded maximum number of env variables")
        }

        let process_id = ctx.local.process_id;
        ctx.global.passthrough_process_id = process_id;
        ctx.copy_exec_fds();
        hook_macros::real!(execvpe)(file, argv, ptr::addr_of!(env) as *const *const libc::c_char)
    }
}

hook_macros::hook! {
     unsafe fn system(command: *const libc::c_char) -> libc::c_int => fizzle_system(_ctx) {
        // env is inherited, so no variables need to be defined
        let fizzle_memory = env::var(FIZZLE_MEMORY_ENV).unwrap();
        let ld_preload = env::var("LD_PRELOAD").unwrap();

        env::remove_var(FIZZLE_MEMORY_ENV);
        env::remove_var("LD_PRELOAD");
        let res = hook_macros::real!(system)(command); // `system` commands are executed without any Fizzle harness
        env::set_var("LD_PRELOAD", ld_preload);
        env::set_var(FIZZLE_MEMORY_ENV, fizzle_memory);

        res
    }
}

hook_macros::hook! {
    unsafe fn exit(status: libc::c_int) => fizzle_exit(ctx) {
        log::warn!("exit called with status {}", status);
        //if ctx.local.suspend_on_exit {
            // TODO: clean up any polling contexts here so that this process never gets
            // delegated to (other than for the purpose of running modules)

            // Temporary hack: whenever processes get delegated to here, just pass back to
            // another process (i.e. ignore inputs)
        drop(ctx);
        loop {
            state::FIZZLE_STATE.yield_thread()
        }
    }
}

hook_macros::hook! {
    unsafe fn _exit(status: libc::c_int) => fizzle_exit2(ctx) {
        log::warn!("_exit called with status {}", status);
        drop(ctx);
        loop {
            state::FIZZLE_STATE.yield_thread()
        }
    }
}

// We need this to ensure that our `atexit` hook is called first when FIZZLE_NOEXIT is set.
hook_macros::hook! {
    unsafe fn atexit(cb: extern "C" fn()) => fizzle_atexit(_ctx) {
        hook_macros::real!(atexit)(cb)
    }
}
