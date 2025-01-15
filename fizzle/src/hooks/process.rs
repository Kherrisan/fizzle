//! Process creation shims.
//!
//!

use std::env;
use std::ffi::{CStr, CString};

use fizzle_common::path::FilePath;

use crate::constants::{FIZZLE_ALLOC_ENV, FIZZLE_MEMORY_ENV};
use crate::errno::Errno;
use crate::handlers::descriptor::Descriptor;
use crate::handlers::id::*;
use crate::handlers::process::*;
use crate::handlers::signal::*;
use crate::hook_macros;
use crate::scheduler::{fizzle_singleton, Scheduler};

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
    env.push(
        CString::new(format!(
            "{}={}",
            FIZZLE_ALLOC_ENV,
            env::var(FIZZLE_ALLOC_ENV).unwrap()
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
        env.push(CString::new(format!("{}={}", FIZZLE_ALLOC_ENV, env::var(FIZZLE_ALLOC_ENV).unwrap())).unwrap());

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
        env.push(CString::new(format!("{}={}", FIZZLE_ALLOC_ENV, env::var(FIZZLE_ALLOC_ENV).unwrap())).unwrap());

        let file_cstr = unsafe { CStr::from_ptr(pathname) };

        crate::strace!("execveat(dirfd={}, pathname={:?}, args={:?}, env={:?}) -> ...", dirfd, file_cstr, args, env);

        if dirfd == libc::AT_FDCWD {
            unimplemented!("`execveat()` dirfd=FD_ATCWD"); // TODO: handle this edge case
        }

        if flags != 0 {
            unimplemented!("`execveat()` AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW flags")
        }

        let file = match FilePath::from_cstr(file_cstr) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Error while parsing `execveat()` pathname: {:?}", e);
                crate::strace!("execveat(dirfd={}, pathname={:?}, args={:?}, env={:?}) -> -1 (EINVAL)", dirfd, file_cstr, args, env);
                Errno::EINVAL.set_errno();
                return -1
            }
        };

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::AtDirectory(Descriptor::from_raw_fd(dirfd), file), Some(env), args)) {
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
        env.push(CString::new(format!("{}={}", FIZZLE_ALLOC_ENV, env::var(FIZZLE_ALLOC_ENV).unwrap())).unwrap());

        crate::strace!("fexecve(fd={}, args={:?}, env={:?}) -> ...", fd, args, env);

        match Scheduler::handle_event(&mut ctx, ProcessExecEvent::new(ExecLocation::Descriptor(Descriptor::from_raw_fd(fd)), Some(env), args)) {
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
        env.push(CString::new(format!("{}={}", FIZZLE_ALLOC_ENV, env::var(FIZZLE_ALLOC_ENV).unwrap())).unwrap());

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
        let fizzle_alloc = env::var(FIZZLE_ALLOC_ENV).unwrap();
        let ld_preload = env::var("LD_PRELOAD").unwrap();

        env::remove_var(FIZZLE_MEMORY_ENV);
        env::remove_var(FIZZLE_ALLOC_ENV);
        env::remove_var("LD_PRELOAD");
        let res = hook_macros::real!(system)(command); // `system` commands are executed without any Fizzle harness
        env::set_var("LD_PRELOAD", ld_preload);
        env::set_var(FIZZLE_ALLOC_ENV, fizzle_alloc);
        env::set_var(FIZZLE_MEMORY_ENV, fizzle_memory);

        res
    }
}

hook_macros::hook! {
    unsafe fn exit(status: libc::c_int) => fizzle_exit(ctx) {
        crate::strace!("exit(status={}) -> !", status);
        let _ = Scheduler::handle_event(&mut ctx, ProcessExitEvent::new(status, true));
        panic!("exit() failed to exit")
    }
}

hook_macros::hook! {
    unsafe fn _exit(status: libc::c_int) => fizzle_exit2(ctx) {
        crate::strace!("_exit(status={}) -> !", status);
        let _ = Scheduler::handle_event(&mut ctx, ProcessExitEvent::new(status, false));
        panic!("_exit() failed to exit")
    }
}

hook_macros::hook! {
    unsafe fn atexit(cb: AtExitFunction) -> libc::c_int => fizzle_atexit(ctx) {
        crate::strace!("atexit(cb={:?}) -> ...", cb);
        match Scheduler::handle_event(&mut ctx, ProcessAtExitEvent::new(cb)) {
            Ok(()) => {
                crate::strace!("atexit(cb={:?}) -> 0", cb);
                0
            }
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn on_exit(cb: OnExitFunction, arg: *mut libc::c_void) -> libc::c_int => fizzle_on_exit(ctx) {
        crate::strace!("on_exit(cb={:?}, arg={:?}) -> ...", cb, arg);
        match Scheduler::handle_event(&mut ctx, ProcessOnExitEvent::new(cb, arg)) {
            Ok(()) => {
                crate::strace!("on_exit(cb={:?}, arg={:?}) -> 0", cb, arg);
                0
            }
            Err(()) => unreachable!(),
        }
    }
}

// TODO: register *real* atexit handler to deal with the case where a process exits naturally

// TODO: interpose c++ quick_exit and on_quick_exit functions

hook_macros::hook! {
    unsafe fn wait(wstatus: *mut libc::c_int) -> libc::pid_t => fizzle_wait(ctx) {
        crate::strace!("wait(wstatus={:?}) -> ...", wstatus);
        match Scheduler::handle_event(&mut ctx, ProcessWaitEvent::new(WaitType::AllChildren, WaitOptions::empty())) {
            Ok(Some(wait_info)) => {
                crate::strace!("wait(wstatus={:?}) -> {}", wstatus, wait_info.pid);

                if !wstatus.is_null() {
                    *wstatus = match wait_info.code {
                        SigChildCode::Continued => 0xffff,
                        SigChildCode::Exited => ((wait_info.status & 0xff) << 8) | 0x00,
                        SigChildCode::Killed => wait_info.status & 0x7f,
                        SigChildCode::Dumped => ((wait_info.status & 0xff) << 8) | 0x80,
                        SigChildCode::Stopped => ((wait_info.status & 0xff) << 8) | 0x7f,
                        SigChildCode::Trapped => ((wait_info.status & 0xff) << 8) | 0x7f, // Same as stopped (TODO: check)
                    };
                }

                wait_info.pid
            },
            Ok(None) => {
                crate::strace!("wait(wstatus={:?}) -> -1 (ECHILD)", wstatus);
                Errno::ECHILD.set_errno();
                -1
            }
            Err(e) => {
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn waitpid(
        pid: libc::pid_t,
        wstatus: *mut libc::c_int,
        options: libc::c_int
    ) -> libc::pid_t => fizzle_waitpid(ctx) {
        let options = WaitOptions::from_bits_truncate(options & (libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED));

        let wait_type = match pid {
            ..=-2 => WaitType::Gid(Pgid::from_raw(-pid)),
            -1 => WaitType::AllChildren,
            0 => {
                let pgid = Scheduler::handle_event(&mut ctx, ProcessGetGroupIdEvent::new(None)).unwrap();
                WaitType::Gid(pgid)
            }
            1.. => WaitType::Pid(Pid::from_raw(pid)),
        };

        crate::strace!("waitpid(pid={}, wstatus={:?}, options={:?}) -> ...", pid, wstatus, options);
        match Scheduler::handle_event(&mut ctx, ProcessWaitEvent::new(wait_type, options)) {
            Ok(Some(wait_info)) => {
                crate::strace!("waitpid(pid={}, wstatus={:?}, options={:?}) -> {}", pid, wstatus, options, wait_info.pid);

                if !wstatus.is_null() {
                    *wstatus = match wait_info.code {
                        SigChildCode::Continued => 0xffff,
                        SigChildCode::Exited => ((wait_info.status & 0xff) << 8) | 0x00,
                        SigChildCode::Killed => wait_info.status & 0x7f,
                        SigChildCode::Dumped => ((wait_info.status & 0xff) << 8) | 0x80,
                        SigChildCode::Stopped => ((wait_info.status & 0xff) << 8) | 0x7f,
                        SigChildCode::Trapped => ((wait_info.status & 0xff) << 8) | 0x7f, // Same as stopped (TODO: check)
                    };
                }

                wait_info.pid
            },
            Ok(None) => {
                crate::strace!("waitpid(pid={}, wstatus={:?}, options={:?}) -> 0", pid, wstatus, options);
                0
            }
            Err(e) => {
                crate::strace!("waitpid(pid={}, wstatus={:?}, options={:?}) -> -1 ({})", pid, wstatus, options, e);
                e.set_errno();
                -1
            }
        }
    }
}

hook_macros::hook! {
    unsafe fn waitid(
        idtype: libc::idtype_t,
        id: libc::id_t,
        infop: *mut siginfo_t,
        options: libc::c_int
    ) -> libc::c_int => fizzle_waitid(ctx) {
        let options = WaitOptions::from_bits_truncate(options);

        let wait_type = match idtype {
            libc::P_PID => WaitType::Pid(Pid::from_raw(id as libc::pid_t)),
            libc::P_PIDFD => WaitType::PidFd(Descriptor::from_raw_fd(id as i32)),
            libc::P_PGID => WaitType::Gid(Pgid::from_raw(id as libc::pid_t)),
            libc::P_ALL => WaitType::AllChildren,
            _ => {
                crate::strace!("waitid(idtype={}, id={}, infop={:?}, options={:?}) -> -1 (EINVAL)", idtype, id, infop, options);
                return -1
            }
        };

        crate::strace!("waitid(idtype={}, id={}, infop={:?}, options={:?}) -> ...", idtype, id, infop, options);
        match Scheduler::handle_event(&mut ctx, ProcessWaitEvent::new(wait_type, options)) {
            Ok(Some(wait_info)) => {
                crate::strace!("waitid(idtype={}, id={}, infop={:?}, options={:?}) -> 0", idtype, id, infop, options);

                if let Some(infop) = unsafe { infop.as_mut() } {
                    *infop = siginfo_t::from_raised(RaisedSignalInfo::Child(wait_info));
                }

                0
            },
            Ok(None) => {
                crate::strace!("waitid(idtype={}, id={}, infop={:?}, options={:?}) -> 0", idtype, id, infop, options);
                0
            }
            Err(e) => {
                crate::strace!("waitid(idtype={}, id={}, infop={:?}, options={:?}) -> -1 ({})", idtype, id, infop, options, e);
                e.set_errno();
                -1
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn clone(
    _f: CloneFunction,
    _stack: *mut libc::c_void,
    _flags: libc::c_int,
    _arg: *mut libc::c_void,
    mut _va_args: ...
) -> libc::c_int {
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `clone()` not allowed");
    }
    crate::state::set_entered_handler(true);

    // Feels more like a thread initially...
    // But also kind of acts more like `fork()`
    unimplemented!("clone");

    // crate::state::set_entered_handler(false);
}

hook_macros::hook! {
    unsafe fn getpid() -> libc::pid_t => fizzle_getpid(ctx) {

        crate::strace!("getpid() -> ...");
        match Scheduler::handle_event(&mut ctx, ProcessGetIdEvent::new()) {
            Ok(pid) => {
                crate::strace!("getpid() -> {}", pid);
                pid
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn getppid() -> libc::pid_t => fizzle_getppid(ctx) {

        crate::strace!("getppid() -> ...");
        match Scheduler::handle_event(&mut ctx, ProcessGetParentIdEvent::new()) {
            Ok(pid) => {
                crate::strace!("getppid() -> {}", pid);
                pid
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn getpgid(pid: libc::pid_t) -> libc::pid_t => fizzle_getpgid(ctx) {
        let worker_id = match pid {
            ..=-1 => {
                Errno::EINVAL.set_errno();
                return -1
            }
            0 => None,
            1.. => Some(Pid::from_raw(pid)),
        };

        crate::strace!("getpgid(pid={}) -> ...", pid);
        match Scheduler::handle_event(&mut ctx, ProcessGetGroupIdEvent::new(worker_id)) {
            Ok(pgid) => {
                let pgid = pgid.as_raw();
                crate::strace!("getpgid(pid={}) -> {}", pid, pgid);
                pgid
            },
            Err(_) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn getpgrp() -> libc::pid_t => fizzle_getpgrp(ctx) {

        crate::strace!("getpgrp() -> ...");
        match Scheduler::handle_event(&mut ctx, ProcessGetGroupIdEvent::new(None)) {
            Ok(pgid) => {
                let pgid = pgid.as_raw();
                crate::strace!("getpgrp() -> {}", pgid);
                pgid
            },
            Err(_) => unreachable!()
        }
    }
}

hook_macros::hook! {
    unsafe fn setpgid(pid: libc::pid_t, pgid: libc::pid_t) -> libc::c_int => fizzle_setpgid(ctx) {
        let worker_id = match pid {
            ..=-1 => {
                Errno::EINVAL.set_errno();
                return -1
            }
            0 => None,
            1.. => Some(Pid::from_raw(pid)),
        };

        crate::strace!("setpgid(pid={}, pgid={}) -> ...", pid, pgid);
        match Scheduler::handle_event(&mut ctx, ProcessSetGroupIdEvent::new(worker_id, Pgid::from_raw(pgid))) {
            Ok(()) => {
                crate::strace!("setpgid(pid={}, pgid={}) -> 0", pid, pgid);
                0
            },
            Err(e) => {
                crate::strace!("setpgid(pid={}, pgid={}) -> -1 ({})", pid, pgid, e);
                -1
            },
        }
    }
}

// TODO: include pid_* functions here
