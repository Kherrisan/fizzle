use std::{
    collections::{hash_map::Entry, VecDeque},
    mem, thread,
};

use crate::{hook_macros, state};

//libc::syscall(1,2,3,4);

const FUTEX_OP_SHIFT_SET: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_SET;
const FUTEX_OP_SHIFT_ADD: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_ADD;
const FUTEX_OP_SHIFT_OR: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_OR;
const FUTEX_OP_SHIFT_NAND: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_ANDN;
const FUTEX_OP_SHIFT_XOR: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_XOR;

fn format_futex_op(futex_op: libc::c_int) -> &'static str {
    if (futex_op & libc::FUTEX_WAIT) != 0 {
        "FUTEX_WAIT"
    } else if (futex_op & libc::FUTEX_WAKE) != 0 {
        "FUTEX_WAKE"
    } else if (futex_op & libc::FUTEX_FD) != 0 {
        "FUTEX_FD"
    } else if (futex_op & libc::FUTEX_REQUEUE) != 0 {
        "FUTEX_REQUEUE"
    } else if (futex_op & libc::FUTEX_CMP_REQUEUE) != 0 {
        "FUTEX_CMP_REQUEUE"
    } else if (futex_op & libc::FUTEX_WAKE_OP) != 0 {
        "FUTEX_WAKE_OP"
    } else if (futex_op & libc::FUTEX_WAIT_BITSET) != 0 {
        "FUTEX_WAIT_BITSET"
    } else if (futex_op & libc::FUTEX_WAKE_BITSET) != 0 {
        "FUTEX_WAKE_BITSET"
    } else {
        "UNIMPLEMENTED OP"
    }
}

/*


    let res = {
        $hook_fn ( $($v,)* $va_args )
    };

    log::trace!(
        "Function {} returned {:?}", // TODO: add process info in the future
        stringify!($real_fn),
        res
    );
    crate::state::set_entered_handler(false);
    res
*/

// VA_ARGS cannot
struct SyscallReal {
    __private_field: (),
}

static SYSCALL_SINGLETON: SyscallReal = SyscallReal {
    __private_field: (),
};

impl SyscallReal {
    pub fn get(&self) -> unsafe extern "C" fn(libc::c_long, ...) -> libc::c_long {
        use std::sync::Once;

        static mut REAL: *const u8 = 0 as *const u8;
        static mut ONCE: Once = Once::new();

        unsafe {
            ONCE.call_once(|| {
                REAL = hook_macros::ld_preload::dlsym_next(concat!("syscall", "\0"));
            });
            ::std::mem::transmute(REAL)
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn syscall(number: libc::c_long, mut va_args: ...) -> libc::c_long {
    if crate::state::has_entered_handler() {
        panic!("recursive calls to `syscall` not allowed");
    }
    crate::state::set_entered_handler(true);

    log::trace!(
        "Thread {:?} invoked function syscall",
        std::thread::current().id(),
    );

    let mut ctx = state::fizzle_state_singleton();

    let mut state = ctx.acquire();

    let res = 'body: {
        match number {
            libc::SYS_gettid => SYSCALL_SINGLETON.get()(number),
            libc::SYS_getrandom => {
                let buf: *mut libc::c_void = va_args.arg();
                let buflen: libc::size_t = va_args.arg();
                let flags: libc::c_uint = va_args.arg();
                drop(state);
                crate::hooks::entropy::fizzle_getrandom(buf, buflen, flags) as i64
            }
            libc::SYS_futex => {
                let uaddr: *mut u32 = va_args.arg();
                let futex_op: libc::c_int = va_args.arg();
                let val: u32 = va_args.arg();

                log::debug!(
                    "syscall(SYS_futex, {:?}, {} ({}), {}, ...)",
                    uaddr,
                    format_futex_op(futex_op),
                    futex_op,
                    val
                );

                let futex_private_flag = (futex_op & libc::FUTEX_PRIVATE_FLAG) != 0;
                let _futex_clock_realtime = (futex_op & libc::FUTEX_CLOCK_REALTIME) != 0;

                if !futex_private_flag {
                    log::warn!("SYS_futex syscall used non-private futex--fizzle does not currently support process-shared futex operations, so this may cause bugs if used in a multiprocess context");
                }

                // When no futex_op is specified, we assume FUTEX_WAIT (based on gRPC behavior):
                // https://github.com/abseil/abseil-cpp/blob/33dca3ef75533ba0cc9b099b17409bb354344497/absl/synchronization/internal/futex.h#L128
                match futex_op & !(libc::FUTEX_PRIVATE_FLAG | libc::FUTEX_CLOCK_REALTIME) {
                    libc::FUTEX_WAIT => {
                        let timeout: *const libc::timespec = va_args.arg(); // We ignore timeout

                        if *uaddr != val {
                            *libc::__errno_location() = libc::EAGAIN;
                            break 'body -1;
                        }

                        match state.local.futex_waiters.entry(uaddr as *const u32) {
                            Entry::Occupied(mut o) => o.get_mut().push_back((
                                libc::FUTEX_BITSET_MATCH_ANY as u32,
                                thread::current().id(),
                            )),
                            Entry::Vacant(v) => {
                                let mut deque = VecDeque::new();
                                deque.push_back((
                                    libc::FUTEX_BITSET_MATCH_ANY as u32,
                                    thread::current().id(),
                                ));
                                v.insert(deque);
                            }
                        };

                        if !timeout.is_null() && (*timeout).tv_sec == 0 && (*timeout).tv_nsec == 0 {
                            *libc::__errno_location() = libc::ETIMEDOUT;
                            break 'body -1;
                        }

                        drop(state);
                        ctx.yield_thread();

                        0
                    }
                    libc::FUTEX_WAKE => {
                        let Some(queue) = state.local.futex_waiters.get_mut(&(uaddr as *const u32))
                        else {
                            break 'body 0;
                        };

                        let mut awoken_threads = Vec::new();
                        for _ in 0..val {
                            match queue.pop_front() {
                                Some(thread) => awoken_threads.push(thread),
                                None => break,
                            }
                        }

                        let ret = awoken_threads.len() as libc::c_long;

                        for (_, thread) in awoken_threads {
                            state.mark_thread_ready(thread);
                        }

                        ret
                    }
                    libc::FUTEX_FD => panic!("FUTEX_FD unimplemented for SYS_futex"),
                    libc::FUTEX_REQUEUE => {
                        let _timeout: *const libc::timespec = va_args.arg();
                        let uaddr2: *mut u32 = va_args.arg();

                        let Some(mut queue) =
                            state.local.futex_waiters.remove(&(uaddr as *const u32))
                        else {
                            break 'body 0;
                        };

                        let mut awoken_threads = Vec::new();
                        for _ in 0..val {
                            match queue.pop_front() {
                                Some(thread) => awoken_threads.push(thread),
                                None => break,
                            }
                        }

                        let ret = (awoken_threads.len() + queue.len()) as libc::c_long;

                        match state.local.futex_waiters.entry(uaddr2 as *const u32) {
                            Entry::Occupied(mut o) => o.get_mut().extend(queue),
                            Entry::Vacant(v) => {
                                v.insert(queue);
                            }
                        }

                        ret
                    }
                    libc::FUTEX_CMP_REQUEUE => {
                        let _timeout: *const libc::timespec = va_args.arg();
                        let uaddr2: *mut u32 = va_args.arg();
                        let val3: u32 = va_args.arg();

                        if *uaddr != val3 {
                            *libc::__errno_location() = libc::EAGAIN;
                            break 'body -1;
                        }

                        let Some(mut queue) =
                            state.local.futex_waiters.remove(&(uaddr as *const u32))
                        else {
                            return 0;
                        };

                        let mut awoken_threads = Vec::new();
                        for _ in 0..val {
                            match queue.pop_front() {
                                Some(thread) => awoken_threads.push(thread),
                                None => break,
                            }
                        }

                        let ret = (awoken_threads.len() + queue.len()) as libc::c_long;

                        match state.local.futex_waiters.entry(uaddr2 as *const u32) {
                            Entry::Occupied(mut o) => o.get_mut().extend(queue),
                            Entry::Vacant(v) => {
                                v.insert(queue);
                            }
                        }

                        ret
                    }
                    libc::FUTEX_WAKE_OP => {
                        let timeout: *const libc::timespec = va_args.arg();
                        let uaddr2: *mut u32 = va_args.arg();
                        let val3: u32 = va_args.arg();

                        // Convert timeout to val2
                        let val2 = u32::from_le_bytes(
                            (*(timeout as *const [u8; mem::size_of::<libc::timespec>()]))
                                [mem::size_of::<libc::timespec>() - 4..]
                                .try_into()
                                .unwrap(),
                        );

                        let oldval = *uaddr2;

                        let op = val3 >> 28;
                        let oparg = (val3 >> 12) & 0b1111_1111_1111;

                        match op as i32 {
                            libc::FUTEX_OP_SET => *uaddr2 = oparg,
                            libc::FUTEX_OP_ADD => *uaddr2 += oparg,
                            libc::FUTEX_OP_OR => *uaddr2 |= oparg,
                            libc::FUTEX_OP_ANDN => *uaddr2 &= !oparg,
                            libc::FUTEX_OP_XOR => *uaddr ^= oparg,
                            FUTEX_OP_SHIFT_SET => *uaddr2 = 1 << oparg,
                            FUTEX_OP_SHIFT_ADD => *uaddr2 += 1 << oparg,
                            FUTEX_OP_SHIFT_OR => *uaddr2 |= 1 << oparg,
                            FUTEX_OP_SHIFT_NAND => *uaddr2 &= !(1 << oparg),
                            FUTEX_OP_SHIFT_XOR => *uaddr ^= 1 << oparg,
                            5..=7 | 13..=15 => {
                                log::warn!("futex syscall had unrecognized op in FUTEX_WAKE_OP");
                                *libc::__errno_location() = libc::EINVAL;
                                break 'body -1;
                            }
                            _ => unreachable!(),
                        }

                        let woken_1 = if let Some(queue) =
                            state.local.futex_waiters.get_mut(&(uaddr as *const u32))
                        {
                            let mut awoken_threads = Vec::new();
                            for _ in 0..val {
                                match queue.pop_front() {
                                    Some(thread) => awoken_threads.push(thread),
                                    None => break,
                                }
                            }

                            let ret = awoken_threads.len() as libc::c_long;

                            for (_, thread) in awoken_threads {
                                state.mark_thread_ready(thread);
                            }

                            ret
                        } else {
                            0
                        };

                        let cmp = (val3 >> 24) & 0b1111;
                        let cmparg = val3 & 0b1111_1111_1111;

                        let should_wake = match cmp as i32 {
                            libc::FUTEX_OP_CMP_EQ => oldval == cmparg,
                            libc::FUTEX_OP_CMP_NE => oldval != cmparg,
                            libc::FUTEX_OP_CMP_LT => oldval < cmparg,
                            libc::FUTEX_OP_CMP_LE => oldval <= cmparg,
                            libc::FUTEX_OP_CMP_GT => oldval > cmparg,
                            libc::FUTEX_OP_CMP_GE => oldval >= cmparg,
                            _ => {
                                log::warn!("futex syscall had unrecognized cmp in FUTEX_WAKE_OP");
                                panic!("FUTEX_WAKE_OP failed")
                            }
                        };

                        if should_wake {
                            let woken_2 = if let Some(queue) =
                                state.local.futex_waiters.get_mut(&(uaddr2 as *const u32))
                            {
                                let mut awoken_threads = Vec::new();
                                for _ in 0..val2 {
                                    match queue.pop_front() {
                                        Some(thread) => awoken_threads.push(thread),
                                        None => break,
                                    }
                                }

                                let ret = awoken_threads.len() as libc::c_long;

                                for (_, thread) in awoken_threads {
                                    state.mark_thread_ready(thread);
                                }

                                ret
                            } else {
                                0
                            };

                            woken_1 + woken_2
                        } else {
                            woken_1
                        }
                    }
                    libc::FUTEX_WAIT_BITSET => {
                        let timeout: *const libc::timespec = va_args.arg();
                        let _uaddr2: *mut u32 = va_args.arg();
                        let val3: u32 = va_args.arg();

                        if *uaddr != val {
                            *libc::__errno_location() = libc::EAGAIN;
                            break 'body -1;
                        }

                        match state.local.futex_waiters.entry(uaddr as *const u32) {
                            Entry::Occupied(mut o) => {
                                o.get_mut().push_back((val3, thread::current().id()))
                            }
                            Entry::Vacant(v) => {
                                let mut deque = VecDeque::new();
                                deque.push_back((val3, thread::current().id()));
                                v.insert(deque);
                            }
                        };

                        if !timeout.is_null() && (*timeout).tv_sec == 0 && (*timeout).tv_nsec == 0 {
                            *libc::__errno_location() = libc::ETIMEDOUT;
                            break 'body -1;
                        }

                        drop(state);
                        ctx.yield_thread();

                        0
                    }

                    libc::FUTEX_WAKE_BITSET => {
                        let _timeout: *const libc::timespec = va_args.arg();
                        let _uaddr2: *mut u32 = va_args.arg();
                        let val3: u32 = va_args.arg();

                        let Some(queue) = state.local.futex_waiters.get_mut(&(uaddr as *const u32))
                        else {
                            break 'body 0;
                        };

                        let mut awoken_threads = Vec::new();

                        for _ in 0..queue.len() {
                            match queue.pop_front() {
                                Some((bitmap, thread)) if (bitmap & val3) != 0 => {
                                    awoken_threads.push(thread)
                                }
                                Some(entry) => queue.push_back(entry),
                                None => break,
                            }
                        }

                        let ret = awoken_threads.len() as libc::c_long;

                        if queue.is_empty() {
                            state.local.futex_waiters.remove(&(uaddr as *const u32));
                        }

                        for thread in awoken_threads {
                            state.mark_thread_ready(thread);
                        }

                        ret
                    }
                    libc::FUTEX_LOCK_PI => unimplemented!(),
                    libc::FUTEX_LOCK_PI2 => unimplemented!(),
                    libc::FUTEX_TRYLOCK_PI => unimplemented!(),
                    libc::FUTEX_UNLOCK_PI => unimplemented!(),
                    libc::FUTEX_WAIT_REQUEUE_PI => unimplemented!(),
                    libc::FUTEX_CMP_REQUEUE_PI => unimplemented!(),
                    _ => panic!("SYS_futex syscall with unrecognized `futex_op` argument"),
                }
            }
            _ => panic!("syscall({}, ...) unsupported by Fizzle", number),
        }
    };

    log::trace!(
        "Function syscall returned {:?}", // TODO: add process info in the future
        res
    );
    crate::state::set_entered_handler(false);

    res
}
