use std::time::Duration;
use std::{mem, slice};

use crate::errno::Errno;
use crate::handlers::descriptor::*;
use crate::handlers::entropy::*;
use crate::handlers::futex::*;
use crate::scheduler::Scheduler;
use crate::{hook_macros, WaitDuration};
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

/*
const FUTEX_OP_SHIFT_SET: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_SET;
const FUTEX_OP_SHIFT_ADD: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_ADD;
const FUTEX_OP_SHIFT_OR: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_OR;
const FUTEX_OP_SHIFT_NAND: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_ANDN;
const FUTEX_OP_SHIFT_XOR: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_XOR;
*/

// TODO: add flags
fn futex_op_fmt(futex_op: libc::c_int) -> String {
    let futex_private_flag = (futex_op & libc::FUTEX_PRIVATE_FLAG) != 0;
    let futex_clock_realtime = (futex_op & libc::FUTEX_CLOCK_REALTIME) != 0;
    let futex_op = futex_op & !(libc::FUTEX_PRIVATE_FLAG | libc::FUTEX_CLOCK_REALTIME);

    let futex_opname = match futex_op {
        libc::FUTEX_WAIT => "FUTEX_WAIT",
        libc::FUTEX_WAKE => "FUTEX_WAKE",
        libc::FUTEX_REQUEUE => "FUTEX_REQUEUE",
        libc::FUTEX_CMP_REQUEUE => "FUTEX_CMP_REQUEUE",
        libc::FUTEX_WAKE_OP => "FUTEX_WAKE_OP",
        libc::FUTEX_WAIT_BITSET => "FUTEX_WAIT_BITSET",
        libc::FUTEX_WAKE_BITSET => "FUTEX_WAKE_BITSET",
        libc::FUTEX_FD => "FUTEX_FD",
        libc::FUTEX_LOCK_PI => "FUTEX_LOCK_PI",
        libc::FUTEX_LOCK_PI2 => "FUTEX_LOCK_PI2",
        libc::FUTEX_TRYLOCK_PI => "FUTEX_TRYLOCK_PI",
        libc::FUTEX_UNLOCK_PI => "FUTEX_UNLOCK_PI",
        libc::FUTEX_WAIT_REQUEUE_PI => "FUTEX_WAIT_REQUEUE_PI",
        libc::FUTEX_CMP_REQUEUE_PI => "FUTEX_CMP_REQUEUE_PI",
        _ => "<UNKNOWN_FUTEX_OP>",
    };

    let mut s = futex_opname.to_string();
    if futex_private_flag {
        s += "|FUTEX_PRIV";
    }

    if futex_clock_realtime {
        s += "|FUTEX_CLK_RT";
    }

    s
}

#[no_mangle]
pub unsafe extern "C" fn syscall(number: libc::c_long, mut va_args: ...) -> libc::c_long {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function syscall() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        return match number {
            libc::SYS_statx => {
                let dirfd: libc::c_int = va_args.arg();
                let pathname: *const libc::c_char = va_args.arg();
                let flags: libc::c_int = va_args.arg();
                let mask: libc::c_uint = va_args.arg();
                let statxbuf: *mut libc::statx = va_args.arg();

                hook_macros::real_syscall()(number, dirfd, pathname, flags, mask, statxbuf)
            }
            libc::SYS_futex => {
                let uaddr: *mut u32 = va_args.arg();
                let futex_op: libc::c_int = va_args.arg();
                let val: u32 = va_args.arg();
                let timeout: *const libc::timespec = va_args.arg();
                let uaddr2: *mut u32 = va_args.arg();
                let val3: u32 = va_args.arg();

                hook_macros::real_syscall()(number, uaddr, futex_op, val, timeout, uaddr2, val3)
            }
            _ => {
                log::debug!("syscall({}, ...)", number);

                panic!("recursive calls to `syscall` not allowed")
            }
        };
    };

    let res = match number {
        libc::SYS_read => {
            let fd: libc::c_int = va_args.arg();
            let buf: *mut libc::c_void = va_args.arg();
            let count: libc::size_t = va_args.arg();
            let descriptor_id = Descriptor::from_raw_fd(fd);

            crate::strace!("syscall(SYS_read, {fd}, {buf:?}, {count}) -> ...");
            let data = slice::from_raw_parts_mut(buf.cast::<u8>(), count);

            match Scheduler::handle_event(&mut ctx, DescriptorReadEvent::new(descriptor_id, ReadData::BasicSlice(data))) {
                Ok(read) => {
                    crate::strace!("syscall(SYS_read, {fd}, {buf:?}, {count}) -> {read}");
                    read as i64
                }
                Err(e) => {
                    crate::strace!("syscall(SYS_read, {fd}, {buf:?}, {count}) -> -1 ({e})");
                    e.set_errno();
                    -1
                },
            }
        }
         libc::SYS_write => {
            let fd: libc::c_int = va_args.arg();
            let buf: *const libc::c_void = va_args.arg();
            let count: libc::size_t = va_args.arg();
            let descriptor_id = Descriptor::from_raw_fd(fd);
            let data = slice::from_raw_parts(buf.cast::<u8>(), count);

            crate::strace!("syscall(SYS_write, {fd}, {buf:?}, {count}) -> ...");

            match Scheduler::handle_event(&mut ctx, DescriptorWriteEvent::new(descriptor_id, WriteData::BasicSlice(data))) {
                Ok(written) => {
                    crate::strace!("syscall(SYS_write, {fd}, {buf:?}, {count}) -> {written}");
                    written as i64
                }
                Err(e) => {
                    crate::strace!("syscall(SYS_write, {fd}, {buf:?}, {count}) -> -1 ({e})");
                    e.set_errno();
                    -1
                },
            }
        }       
        libc::SYS_open => {
            let pathname: *const libc::c_char = va_args.arg();
            let flags: libc::c_int = va_args.arg();
            let mode: libc::mode_t = va_args.arg();
            unsafe { libc::open(pathname, flags, mode) as i64 }
        }
        libc::SYS_close => {
            let fd: libc::c_int = va_args.arg();
            let descriptor_id = Descriptor::from_raw_fd(fd);

            crate::strace!("syscall(SYS_close, {fd}) -> ...");

            match Scheduler::handle_event(&mut ctx, DescriptorCloseEvent::new(descriptor_id)) {
                Ok(()) => {
                    crate::strace!("syscall(SYS_close, {fd}) -> 0");
                    0
                }
                Err(e) => {
                    crate::strace!("syscall(SYS_close, {fd}) -> -1 ({e})");
                    e.set_errno();
                    -1
                },
            }
        }
        libc::SYS_io_setup => {
            crate::strace!("syscall(SYS_io_setup, ...) -> ...");

            Errno::ENOSYS.set_errno();
            -1
        }
        libc::SYS_sched_getaffinity => {
            crate::strace!("syscall(SYS_sched_getaffinity, ...) -> ...");
            let pid: libc::pid_t = va_args.arg();
            let cpusetsize: libc::size_t = va_args.arg();
            let mask: *mut libc::cpu_set_t = va_args.arg();

            let res = hook_macros::real_syscall()(number, pid, cpusetsize, mask);
            crate::strace!("syscall(SYS_sched_getaffinity, pid={}, cupusetsize={}, mask={:?}) -> {}", pid, cpusetsize, mask, res);
            res
        }
        libc::SYS_gettid => {
            crate::strace!("syscall(SYS_gettid) -> ...");
            let res = hook_macros::real_syscall()(number);
            crate::strace!("syscall(SYS_gettid) -> {}", res);
            res
        }
        libc::SYS_getrandom => {
            let buf: *mut libc::c_void = va_args.arg();
            let buflen: libc::size_t = va_args.arg();
            let flags: libc::c_uint = va_args.arg();

            crate::strace!(
                "syscall(SYS_getrandom, buf={:?}, buflen={}, flags={}) -> ...",
                buf,
                buflen,
                flags
            );

            let s = slice::from_raw_parts_mut(buf as *mut u8, buflen as usize);
            match Scheduler::handle_event(&mut ctx, GetEntropyEvent::new(s)) {
                Ok(len) => {
                    crate::strace!(
                        "syscall(SYS_getrandom, buf={:?}, buflen={}, flags={}) -> {:.16?}",
                        buf,
                        buflen,
                        flags,
                        &s[..len]
                    );
                    len as i64
                }
                Err(()) => unreachable!(),
            }
        }
        libc::SYS_futex => {
            let uaddr: *mut u32 = va_args.arg();
            let futex_op: libc::c_int = va_args.arg();
            let val: u32 = va_args.arg();

            let futex_private_flag = (futex_op & libc::FUTEX_PRIVATE_FLAG) != 0;
            let _futex_clock_realtime = (futex_op & libc::FUTEX_CLOCK_REALTIME) != 0;
            let futex_op = futex_op & !(libc::FUTEX_PRIVATE_FLAG | libc::FUTEX_CLOCK_REALTIME);

            if !futex_private_flag {
                log::warn!("SYS_futex syscall used non-private futex--fizzle does not currently support process-shared futex operations, so this may cause bugs if used in a multiprocess context");
            }

            match futex_op {
                libc::FUTEX_WAIT => {
                    let timeout_ptr: *const libc::timespec = va_args.arg();
                    let timeout = if timeout_ptr.is_null() {
                        None
                    } else {
                        Some(
                            Duration::from_secs((*timeout_ptr).tv_sec as u64)
                                + Duration::from_secs((*timeout_ptr).tv_nsec as u64),
                        )
                    };

                    let duration = match timeout {
                        None => WaitDuration::Indefinite,
                        Some(t) => WaitDuration::Timed(t),
                    };

                    crate::strace!(
                        "syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={:?}) -> ...",
                        uaddr,
                        futex_op_fmt(futex_op),
                        val,
                        timeout
                    );

                    match Scheduler::handle_event(
                        &mut ctx,
                        FutexWaitEvent::new(&mut *uaddr, val, duration),
                    ) {
                        Ok(()) => {
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={:?}) -> {}", uaddr, futex_op_fmt(futex_op), val, timeout, 0);
                            0
                        }
                        Err(e) => {
                            let ret = e.out();
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={:?}) -> {} ({})", uaddr, futex_op_fmt(futex_op), val, timeout, ret, e.errno());
                            ret
                        }
                    }
                }
                libc::FUTEX_WAKE => {
                    crate::strace!(
                        "syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}) -> ...",
                        uaddr,
                        futex_op_fmt(futex_op),
                        val
                    );

                    match Scheduler::handle_event(&mut ctx, FutexWakeEvent::new(&mut *uaddr, val)) {
                        Ok(ret) => {
                            crate::strace!(
                                "syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}) -> {}",
                                uaddr,
                                futex_op_fmt(futex_op),
                                val,
                                ret
                            );
                            ret as i64
                        }
                        Err(_) => unreachable!(),
                    }
                }
                libc::FUTEX_REQUEUE => {
                    let timeout_ptr: *const libc::timespec = va_args.arg();
                    let uaddr2: *mut u32 = va_args.arg();
                    let timeout = if timeout_ptr.is_null() {
                        None
                    } else {
                        Some(*timeout_ptr)
                    };

                    let timeout_fmt = match timeout {
                        None => "<null>".to_string(),
                        Some(t) => format!("{}.{:09}", t.tv_sec, t.tv_nsec),
                    };

                    crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={}, uaddr2={:?}) -> ...", uaddr, futex_op_fmt(futex_op), val, timeout_fmt, uaddr2);

                    match Scheduler::handle_event(
                        &mut ctx,
                        FutexRequeueEvent::new(&mut *uaddr, val, timeout, &mut *uaddr2),
                    ) {
                        Ok(ret) => {
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={}, uaddr2={:?}) -> {}", uaddr, futex_op_fmt(futex_op), val, timeout_fmt, uaddr2, ret);
                            ret as i64
                        }
                        Err(_) => unreachable!(),
                    }
                }
                libc::FUTEX_CMP_REQUEUE => {
                    let timeout_ptr: *const libc::timespec = va_args.arg();
                    let uaddr2: *mut u32 = va_args.arg();
                    let val3: u32 = va_args.arg();

                    let timeout = if timeout_ptr.is_null() {
                        None
                    } else {
                        Some(*timeout_ptr)
                    };

                    let timeout_fmt = match timeout {
                        None => "<null>".to_string(),
                        Some(t) => format!("{}.{:09}", t.tv_sec, t.tv_nsec),
                    };

                    crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={}, uaddr2={:?}, val3={}) -> ...", uaddr, futex_op_fmt(futex_op), val, timeout_fmt, uaddr2, val3);

                    match Scheduler::handle_event(
                        &mut ctx,
                        FutexRequeueEvent::new(&mut *uaddr, val, timeout, &mut *uaddr2),
                    ) {
                        Ok(ret) => {
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={}, uaddr2={:?}, val3={}) -> {}", uaddr, futex_op_fmt(futex_op), val, timeout_fmt, uaddr2, val3, ret);
                            ret as i64
                        }
                        Err(_) => unreachable!(),
                    }
                }
                libc::FUTEX_WAKE_OP => {
                    let timeout_ptr: *const libc::timespec = va_args.arg();
                    let uaddr2: *mut u32 = va_args.arg();
                    let val3: u32 = va_args.arg();

                    // Convert timeout to val2
                    let val2 = u32::from_le_bytes(
                        (&(*(timeout_ptr as *const [u8; mem::size_of::<libc::timespec>()])))
                            [mem::size_of::<libc::timespec>() - 4..]
                            .try_into()
                            .unwrap(),
                    );

                    crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, val2={}, uaddr2={:?}, val3={}) -> ...", uaddr, futex_op_fmt(futex_op), val, val2, uaddr2, val3);

                    match Scheduler::handle_event(
                        &mut ctx,
                        FutexWakeOpEvent::new(&mut *uaddr, val, val2, &mut *uaddr2, val3),
                    ) {
                        Ok(ret) => {
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, val2={}, uaddr2={:?}, val3={}) -> {}", uaddr, futex_op_fmt(futex_op), val, val2, uaddr2, val3, ret);
                            ret as i64
                        }
                        Err(e) => {
                            let ret = e.out();
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, val2={}, uaddr2={:?}, val3={}) -> {} ({}", uaddr, futex_op_fmt(futex_op), val, val2, uaddr2, val3, ret, e.errno());
                            ret
                        }
                    }
                }
                libc::FUTEX_WAIT_BITSET => {
                    let timeout_ptr: *const libc::timespec = va_args.arg();
                    let uaddr2: *mut u32 = va_args.arg();
                    let val3: u32 = va_args.arg();

                    let timeout = if timeout_ptr.is_null() {
                        None
                    } else {
                        Some(
                            Duration::from_secs((*timeout_ptr).tv_sec as u64)
                                + Duration::from_secs((*timeout_ptr).tv_nsec as u64),
                        )
                    };

                    let duration = match timeout {
                        None => WaitDuration::Indefinite,
                        Some(t) => WaitDuration::Timed(t),
                    };

                    crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={:?}, uaddr2={:?}, val3={}) -> ...", uaddr, futex_op_fmt(futex_op), val, timeout, uaddr2, val3);

                    match Scheduler::handle_event(
                        &mut ctx,
                        FutexWaitEvent::new(&mut *uaddr, val, duration),
                    ) {
                        Ok(()) => {
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={:?}, uaddr2:{:?}, val3={}) -> {}", uaddr, futex_op_fmt(futex_op), val, timeout, uaddr2, val3, 0);
                            0
                        }
                        Err(e) => {
                            let ret = e.out();
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, timeout={:?}, uaddr2={:?}, val3={}) -> {} ({})", uaddr, futex_op_fmt(futex_op), val, timeout, uaddr2, val3, ret, e.errno());
                            ret
                        }
                    }
                }

                libc::FUTEX_WAKE_BITSET => {
                    let _timeout: *const libc::timespec = va_args.arg();
                    let _uaddr2: *mut u32 = va_args.arg();
                    let val3: u32 = va_args.arg();

                    crate::strace!(
                        "syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, val3={}) -> ...",
                        uaddr,
                        futex_op_fmt(futex_op),
                        val,
                        val3
                    );

                    match Scheduler::handle_event(
                        &mut ctx,
                        FutexWakeBitsetEvent::new(&mut *uaddr, val, val3),
                    ) {
                        Ok(ret) => {
                            crate::strace!("syscall(SYS_futex, uaddr={:?}, futex_op={}, val={}, val3={}) -> {}", uaddr, futex_op_fmt(futex_op), val, val3, ret);
                            ret as i64
                        }
                        Err(_) => unreachable!(),
                    }
                }
                libc::FUTEX_FD => {
                    panic!("syscall SYS_futex with FUTEX_FD has been deprecated since Linux 2.6")
                }
                libc::FUTEX_LOCK_PI => unimplemented!("FUTEX_LOCK_PI"),
                libc::FUTEX_LOCK_PI2 => unimplemented!("FUTEX_LOCK_PI2"),
                libc::FUTEX_TRYLOCK_PI => unimplemented!("FUTEX_TRYLOCK_PI"),
                libc::FUTEX_UNLOCK_PI => unimplemented!("FUTEX_UNLOCK_PI"),
                libc::FUTEX_WAIT_REQUEUE_PI => unimplemented!("FUTEX_WAIT_REQUEUE_PI"),
                libc::FUTEX_CMP_REQUEUE_PI => unimplemented!("FUTEX_CMP_REQUEUE_PI"),
                _ => panic!("SYS_futex syscall with unrecognized `futex_op` argument"),
            }
        }
        libc::SYS_membarrier => {
            let cmd: libc::c_int = va_args.arg();
            let flags: libc::c_int = va_args.arg();
            hook_macros::real_syscall()(libc::SYS_membarrier, cmd, flags)
        }
        libc::SYS_mprotect => {
            let addr: *const libc::c_void = va_args.arg();
            let len: libc::size_t = va_args.arg();
            let prot: libc::c_int = va_args.arg();
            hook_macros::real_syscall()(libc::SYS_mprotect, addr, len, prot)
        }
        libc::SYS_capget => {
            let hdrp: *const libc::c_void = va_args.arg();
            let datap: *const libc::c_void = va_args.arg();
            hook_macros::real_syscall()(libc::SYS_capget, hdrp, datap)
        }
        libc::SYS_capset => {
            let hdrp: *const libc::c_void = va_args.arg();
            let datap: *const libc::c_void = va_args.arg();
            hook_macros::real_syscall()(libc::SYS_capset, hdrp, datap)
        }
        _ => panic!("syscall({}, ...) unsupported by Fizzle", number),
    };

    drop(ctx);
    crate::hooks::post_hook();

    res
}
