//! Process creation shims.
//!
//!

use std::env;
use std::ffi::{CStr, CString};

use fizzle_common::path::FilePath;

use crate::constants::FIZZLE_MEMORY_ENV;
use crate::errno::Errno;
use crate::handlers::descriptor::DescriptorId;
use crate::handlers::process::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;
use crate::state::fizzle_singleton;

const MAX_ARGS: usize = 512;

pub type CloneFunction = unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int;

hook_macros::hook! {
    unsafe fn fork() -> libc::pid_t => fizzle_fork(ctx) {
        crate::strace!("fork() -> ...");
        match Scheduler::handle_event(&mut ctx, ProcessForkEvent::new()) {
            Ok(pid) => {
                crate::strace!("fork() -> {}", pid);
                0
            },
            Err(e) => {
                crate::strace!("fork() -> -1 ({})", e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn vfork() -> libc::pid_t => fizzle_vfork(ctx) {
        // TODO: set `vfork` local flag to check for UB

        crate::strace!("vfork() -> ...");
        match Scheduler::handle_event(&mut ctx, ProcessForkEvent::new()) {
            Ok(pid) => {
                crate::strace!("vfork() -> {}", pid);
                0
            },
            Err(e) => {
                crate::strace!("vfork() -> -1 ({})", e);
                e.set_errno();
                -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn pthread_atfork(
        prepare: Option<unsafe extern "C" fn()>,
        parent: Option<unsafe extern "C" fn()>,
        child: Option<unsafe extern "C" fn()>
    ) -> libc::c_int => fizzle_pthread_atfork(ctx) {

        let atfork_info = AtForkInfo {
            prepare,
            parent,
            child,
        };

        crate::strace!("pthread_atfork(prepare={:?}, parent={:?}, child={:?}) -> ...", prepare, parent, child);
        match Scheduler::handle_event(&mut ctx, RegisterAtForkEvent::new(atfork_info)) {
            Ok(()) => {
                crate::strace!("pthread_atfork(prepare={:?}, parent={:?}, child={:?}) -> 0", prepare, parent, child);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn execl(
    pathname: *const libc::c_char,
    arg: *const libc::c_char,
    mut va_args: ...
) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `execl` not allowed");
    }
    crate::state::set_entered_handler(true);

    let mut ctx = fizzle_singleton();

    let mut args = vec![unsafe { CStr::from_ptr(arg).to_owned() }];
    loop {
        let arg: *const libc::c_char = va_args.arg();
        if arg.is_null() {
            break;
        }
        args.push(unsafe { CStr::from_ptr(pathname).to_owned() });
    }

    let file_cstr = unsafe { CStr::from_ptr(pathname) };

    crate::strace!("execl(pathname={:?}, args={:?}) -> ...", file_cstr, args);

    let file = match FilePath::from_cstr(file_cstr) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Error while parsing `execl()` pathname: {:?}", e);
            crate::strace!(
                "execl(pathname={:?}, args={:?}) -> -1 (EINVAL)",
                file_cstr,
                args
            );
            Errno::EINVAL.set_errno();

            crate::state::set_entered_handler(false);
            return -1;
        }
    };

    match Scheduler::handle_event(
        &mut ctx,
        ProcessExecEvent::new(ExecLocation::File(file), None, args),
    ) {
        Ok(()) => unreachable!(),
        Err(e) => {
            crate::strace!("execl(pathname={:?}, ...) -> -1 ({})", file_cstr, e);
            e.set_errno();

            crate::state::set_entered_handler(false);
            return -1;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn execlp(
    pathname: *const libc::c_char,
    arg: *const libc::c_char,
    mut va_args: ...
) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `execlp` not allowed");
    }
    crate::state::set_entered_handler(true);

    let mut ctx = fizzle_singleton();

    let mut args = vec![unsafe { CStr::from_ptr(arg).to_owned() }];
    loop {
        let arg: *const libc::c_char = va_args.arg();
        if arg.is_null() {
            break;
        }
        args.push(unsafe { CStr::from_ptr(pathname).to_owned() });
    }

    let file_cstr = unsafe { CStr::from_ptr(pathname) };

    crate::strace!("execlp(pathname={:?}, args={:?}) -> ...", file_cstr, args);

    let file = match FilePath::from_cstr(file_cstr) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Error while parsing `execlp()` pathname: {:?}", e);
            crate::strace!(
                "execlp(pathname={:?}, args={:?}) -> -1 (EINVAL)",
                file_cstr,
                args
            );
            Errno::EINVAL.set_errno();

            crate::state::set_entered_handler(false);
            return -1;
        }
    };

    match Scheduler::handle_event(
        &mut ctx,
        ProcessExecEvent::new(ExecLocation::ShellFile(file), None, args),
    ) {
        Ok(()) => unreachable!(),
        Err(e) => {
            crate::strace!("execlp(pathname={:?}, ...) -> -1 ({})", file_cstr, e);
            e.set_errno();

            crate::state::set_entered_handler(false);
            return -1;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn execle(
    pathname: *const libc::c_char,
    arg: *const libc::c_char,
    mut va_args: ...
) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `execle` not allowed");
    }
    crate::state::set_entered_handler(true);

    let mut ctx = fizzle_singleton();

    let mut args = vec![unsafe { CStr::from_ptr(arg).to_owned() }];
    loop {
        let arg: *const libc::c_char = va_args.arg();
        if arg.is_null() {
            break;
        }
        args.push(unsafe { CStr::from_ptr(pathname).to_owned() });
    }

    let envp: *const *const libc::c_char = va_args.arg();
    let mut env = Vec::new();
    let mut env_idx = 0;
    loop {
        let e = unsafe { *envp.add(env_idx) };
        if e.is_null() {
            break;
        }

        env.push(unsafe { CStr::from_ptr(e).to_owned() });
        env_idx += 1;
    }

    env.push(CString::new(format!("LD_PRELOAD={}", env::var("LD_PRELOAD").unwrap())).unwrap());
    env.push(
        CString::new(format!(
            "{}={}",
            FIZZLE_MEMORY_ENV,
            env::var(FIZZLE_MEMORY_ENV).unwrap()
        ))
        .unwrap(),
    );

    let file_cstr = unsafe { CStr::from_ptr(pathname) };

    crate::strace!(
        "execle(pathname={:?}, args={:?}, env={:?}) -> ...",
        file_cstr,
        args,
        env
    );

    let file = match FilePath::from_cstr(file_cstr) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Error while parsing `execle()` pathname: {:?}", e);
            crate::strace!(
                "execle(pathname={:?}, args={:?}, env={:?}) -> -1 (EINVAL)",
                file_cstr,
                args,
                env
            );
            Errno::EINVAL.set_errno();

            crate::state::set_entered_handler(false);
            return -1;
        }
    };

    match Scheduler::handle_event(
        &mut ctx,
        ProcessExecEvent::new(ExecLocation::File(file), Some(env), args),
    ) {
        Ok(()) => unreachable!(),
        Err(e) => {
            crate::strace!("execlp(pathname={:?}, ...) -> -1 ({})", file_cstr, e);
            e.set_errno();

            crate::state::set_entered_handler(false);
            return -1;
        }
    }
}

hook_macros::hook! {
    unsafe fn execv(pathname: *const libc::c_char, argv: *const *const libc::c_char) -> libc::c_int => fizzle_execv(ctx) {
        let mut args = Vec::new();
        let mut arg_idx = 0;
        loop {
            let e = unsafe { *argv.add(arg_idx) };
            if e.is_null() {
                break
            }

            args.push(unsafe { CStr::from_ptr(e).to_owned() });
            arg_idx += 1;
        }

        let file_cstr = unsafe { CStr::from_ptr(pathname) };

        crate::strace!("execv(pathname={:?}, args={:?}) -> ...", file_cstr, args);

        let file = match FilePath::from_cstr(file_cstr) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Error while parsing `execv()` pathname: {:?}", e);
                crate::strace!("execv(pathname={:?}, args={:?}) -> -1 (EINVAL)", file_cstr, args);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::File(file), None, args)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("execv(pathname={:?}, ...) -> -1 ({})", file_cstr, e);
                e.set_errno();
                return -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn execvp(file: *const libc::c_char, argv: *const *const libc::c_char) -> libc::c_int => fizzle_execvp(ctx) {
        let mut args = Vec::new();
        let mut arg_idx = 0;
        loop {
            let e = unsafe { *argv.add(arg_idx) };
            if e.is_null() {
                break
            }

            args.push(unsafe { CStr::from_ptr(e).to_owned() });
            arg_idx += 1;
        }

        let file_cstr = unsafe { CStr::from_ptr(file) };

        crate::strace!("execvp(file={:?}, args={:?}) -> ...", file_cstr, args);

        let file = match FilePath::from_cstr(file_cstr) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Error while parsing `execvp()` file: {:?}", e);
                crate::strace!("execvp(file={:?}, args={:?}) -> -1 (EINVAL)", file_cstr, args);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::ShellFile(file), None, args)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("execvp(file={:?}, ...) -> -1 ({})", file_cstr, e);
                e.set_errno();
                return -1
            },
        }
    }
}

hook_macros::hook! {
     unsafe fn execve(pathname: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_execve(ctx) {
        let mut args = Vec::new();
        let mut arg_idx = 0;
        loop {
            let e = unsafe { *argv.add(arg_idx) };
            if e.is_null() {
                break
            }

            args.push(unsafe { CStr::from_ptr(e).to_owned() });
            arg_idx += 1;
        }

        let mut env = Vec::new();
        let mut env_idx = 0;
        loop {
            let e = unsafe { *envp.add(env_idx) };
            if e.is_null() {
                break
            }

            env.push(unsafe { CStr::from_ptr(e).to_owned() });
            env_idx += 1;
        }

        env.push(CString::new(format!("LD_PRELOAD={}", env::var("LD_PRELOAD").unwrap())).unwrap());
        env.push(CString::new(format!("{}={}", FIZZLE_MEMORY_ENV, env::var(FIZZLE_MEMORY_ENV).unwrap())).unwrap());

        let file_cstr = unsafe { CStr::from_ptr(pathname) };

        crate::strace!("execve(pathname={:?}, args={:?}, env={:?}) -> ...", file_cstr, args, env);

        let file = match FilePath::from_cstr(file_cstr) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Error while parsing `execve()` pathname: {:?}", e);
                crate::strace!("execve(pathname={:?}, args={:?}, env={:?}) -> -1 (EINVAL)", file_cstr, args, env);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::File(file), Some(env), args)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("execve(pathname={:?}, ...) -> -1 ({})", file_cstr, e);
                e.set_errno();
                return -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn execveat(dirfd: libc::c_int, pathname: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char, flags: libc::c_int) -> libc::c_int => fizzle_execveat(ctx) {
        let mut args = Vec::new();
        let mut arg_idx = 0;
        loop {
            let e = unsafe { *argv.add(arg_idx) };
            if e.is_null() {
                break
            }

            args.push(unsafe { CStr::from_ptr(e).to_owned() });
            arg_idx += 1;
        }

        let mut env = Vec::new();
        let mut env_idx = 0;
        loop {
            let e = unsafe { *envp.add(env_idx) };
            if e.is_null() {
                break
            }

            env.push(unsafe { CStr::from_ptr(e).to_owned() });
            env_idx += 1;
        }

        env.push(CString::new(format!("LD_PRELOAD={}", env::var("LD_PRELOAD").unwrap())).unwrap());
        env.push(CString::new(format!("{}={}", FIZZLE_MEMORY_ENV, env::var(FIZZLE_MEMORY_ENV).unwrap())).unwrap());

        let file_cstr = unsafe { CStr::from_ptr(pathname) };

        if dirfd == libc::AT_FDCWD {
            panic!("FD_ATCWD unimplemented"); // TODO: handle this edge case
        }

        crate::strace!("execveat(dirfd={}, pathname={:?}, args={:?}, env={:?}) -> ...", dirfd, file_cstr, args, env);

        let file = match FilePath::from_cstr(file_cstr) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Error while parsing `execveat()` pathname: {:?}", e);
                crate::strace!("execveat(dirfd={}, pathname={:?}, args={:?}, env={:?}) -> -1 (EINVAL)", dirfd, file_cstr, args, env);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::AtDirectory(DescriptorId::from_raw_fd(dirfd), file), Some(env), args)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("execveat(dirfd={}, pathname={:?}, ...) -> -1 ({})", dirfd, file_cstr, e);
                e.set_errno();
                return -1
            },
        }
    }
}

hook_macros::hook! {
    unsafe fn fexecve(fd: libc::c_int, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_fexecve(ctx) {
        let mut args = Vec::new();
        let mut arg_idx = 0;
        loop {
            let e = unsafe { *argv.add(arg_idx) };
            if e.is_null() {
                break
            }

            args.push(unsafe { CStr::from_ptr(e).to_owned() });
            arg_idx += 1;
        }

        let mut env = Vec::new();
        let mut env_idx = 0;
        loop {
            let e = unsafe { *envp.add(env_idx) };
            if e.is_null() {
                break
            }

            env.push(unsafe { CStr::from_ptr(e).to_owned() });
            env_idx += 1;
        }

        env.push(CString::new(format!("LD_PRELOAD={}", env::var("LD_PRELOAD").unwrap())).unwrap());
        env.push(CString::new(format!("{}={}", FIZZLE_MEMORY_ENV, env::var(FIZZLE_MEMORY_ENV).unwrap())).unwrap());

        crate::strace!("fexecve(fd={}, args={:?}, env={:?}) -> ...", fd, args, env);

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::Descriptor(DescriptorId::from_raw_fd(fd)), Some(env), args)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("execveat(fd={}, ...) -> -1 ({})", fd, e);
                e.set_errno();
                return -1
            },
        }
    }
}

hook_macros::hook! {
     unsafe fn execvpe(file: *const libc::c_char, argv: *const *const libc::c_char, envp: *const *const libc::c_char) -> libc::c_int => fizzle_execvpe(ctx) {
        let mut args = Vec::new();
        let mut arg_idx = 0;
        loop {
            let e = unsafe { *argv.add(arg_idx) };
            if e.is_null() {
                break
            }

            args.push(unsafe { CStr::from_ptr(e).to_owned() });
            arg_idx += 1;
        }

        let mut env = Vec::new();
        let mut env_idx = 0;
        loop {
            let e = unsafe { *envp.add(env_idx) };
            if e.is_null() {
                break
            }

            env.push(unsafe { CStr::from_ptr(e).to_owned() });
            env_idx += 1;
        }

        env.push(CString::new(format!("LD_PRELOAD={}", env::var("LD_PRELOAD").unwrap())).unwrap());
        env.push(CString::new(format!("{}={}", FIZZLE_MEMORY_ENV, env::var(FIZZLE_MEMORY_ENV).unwrap())).unwrap());

        let file_cstr = unsafe { CStr::from_ptr(file) };

        crate::strace!("execvpe(pathname={:?}, args={:?}, env={:?}) -> ...", file_cstr, args, env);

        let file = match FilePath::from_cstr(file_cstr) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Error while parsing `execvpe()` pathname: {:?}", e);
                crate::strace!("execvpe(pathname={:?}, args={:?}, env={:?}) -> -1 (EINVAL)", file_cstr, args, env);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::ShellFile(file), Some(env), args)) {
            Ok(()) => unreachable!(),
            Err(e) => {
                crate::strace!("execvpe(pathname={:?}, ...) -> -1 ({})", file_cstr, e);
                e.set_errno();
                return -1
            },
        }
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
        //if state.local.suspend_on_exit {
            // TODO: clean up any polling contexts here so that this process never gets
            // delegated to (other than for the purpose of running modules)

            // Temporary hack: whenever processes get delegated to here, just pass back to
            // another process (i.e. ignore inputs)
        loop {
            ctx.yield_thread()
        }
    }
}

hook_macros::hook! {
    unsafe fn _exit(status: libc::c_int) => fizzle_exit2(ctx) {
        log::warn!("_exit called with status {}", status);
        loop {
            ctx.yield_thread()
        }
    }
}

// We need this to ensure that our `atexit` hook is called first when FIZZLE_NOEXIT is set.
hook_macros::hook! {
    unsafe fn atexit(cb: extern "C" fn()) => fizzle_atexit(_ctx) {
        hook_macros::real!(atexit)(cb)
    }
}

hook_macros::hook! {
    unsafe fn wait(_wstatus: *mut libc::c_int) -> libc::pid_t => fizzle_wait(_ctx) {
        panic!("wait() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn waitpid(
        pid: libc::pid_t,
        wstatus: *mut libc::c_int,
        options: libc::c_int
    ) -> libc::pid_t => fizzle_waitpid(_ctx) {
        let no_hang = (options & libc::WNOHANG) > 0;
        let _untraced = (options & libc::WUNTRACED) > 0;
        let _continued = (options & libc::WCONTINUED) > 0;

        if no_hang {
            return hook_macros::real!(waitpid)(pid, wstatus, options)
        }
        panic!("waitpid() unimplemented")
    }
}

hook_macros::hook! {
    unsafe fn waitid(
        _idtype: libc::idtype_t,
        _id: libc::id_t,
        _infop: *mut libc::siginfo_t,
        _options: libc::c_int
    ) -> libc::c_int => fizzle_waitid(_ctx) {
        panic!("waitid() unimplemented")
    }
}

#[no_mangle]
pub unsafe extern "C" fn clone(
    f: CloneFunction,
    stack: *mut libc::c_void,
    flags: libc::c_int,
    arg: *mut libc::c_void,
    mut va_args: ...
) -> libc::c_int {
    // Feels more like a thread initially...
    // But also kind of acts more like `fork()`
    unimplemented!("clone")
}
