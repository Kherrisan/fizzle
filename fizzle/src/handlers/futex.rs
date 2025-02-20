use std::collections::hash_map::Entry;
use std::collections::VecDeque;
use std::{ptr, thread};

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome, YieldUntil};
use crate::state::FizzleState;
use crate::WaitDuration;

const FUTEX_OP_SHIFT_SET: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_SET;
const FUTEX_OP_SHIFT_ADD: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_ADD;
const FUTEX_OP_SHIFT_OR: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_OR;
const FUTEX_OP_SHIFT_NAND: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_ANDN;
const FUTEX_OP_SHIFT_XOR: i32 = libc::FUTEX_OP_OPARG_SHIFT | libc::FUTEX_OP_XOR;

// TODO: some timeouts are absolute (I think FUTEX_WAIT_BITSET?)--handle these

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FutexPtr(usize);

impl FutexPtr {
    pub fn from_mut(m: &mut u32) -> Self {
        Self(ptr::from_ref(m) as usize)
    }
}

impl From<*const u32> for FutexPtr {
    fn from(value: *const u32) -> Self {
        FutexPtr(value as usize)
    }
}

pub enum FutexError {
    TimedOut,
    NotReady,
    InvalidValue,
}

impl FutexError {
    pub fn out(&self) -> i64 {
        unsafe {
            *libc::__errno_location() = self.errno().into();
        }

        -1
    }

    pub fn errno(&self) -> Errno {
        match self {
            Self::TimedOut => Errno::ETIMEDOUT,
            Self::NotReady => Errno::EAGAIN,
            Self::InvalidValue => Errno::EINVAL,
        }
    }
}

#[derive(Clone, Copy)]
enum FutexWaitState {
    Start,
    Finish,
}

pub struct FutexWaitEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
    duration: WaitDuration,
    state: FutexWaitState,
}

impl<'a> FutexWaitEvent<'a> {
    pub fn new(uaddr: &'a mut u32, val: u32, duration: WaitDuration) -> Self {
        Self {
            uaddr,
            val,
            duration,
            state: FutexWaitState::Start,
        }
    }
}

impl Event for FutexWaitEvent<'_> {
    type Success = ();
    type Error = FutexError;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let futex_ptr = FutexPtr::from_mut(self.uaddr);

        match self.state {
            FutexWaitState::Start => {
                self.state = FutexWaitState::Finish;

                if *self.uaddr != self.val {
                    return Outcome::Error(FutexError::NotReady);
                }

                match state.local.futex_waiters.entry(futex_ptr) {
                    Entry::Occupied(mut o) => o
                        .get_mut()
                        .push_back((libc::FUTEX_BITSET_MATCH_ANY as u32, thread::current().id())),
                    Entry::Vacant(v) => {
                        // Create a new Futex location at the specified address
                        let mut deque = VecDeque::new();
                        deque.push_back((
                            libc::FUTEX_BITSET_MATCH_ANY as u32,
                            thread::current().id(),
                        ));
                        v.insert(deque);
                    }
                }

                // Now wait for futex to be unblocked
                match self.duration {
                    WaitDuration::Immediate => unreachable!(), // No such thing as try* in futex semantics
                    WaitDuration::Indefinite => Outcome::Yield(YieldUntil::None),
                    WaitDuration::Timed(duration) => Outcome::Yield(YieldUntil::Reschedule(duration)),
                }
            }
            FutexWaitState::Finish => {
                let Some(waiters) = state.local.futex_waiters.get_mut(&futex_ptr) else {
                    panic!("internal Fizzle error: mutex destroyed while being waited on");
                };

                for (idx, (_, thread_id)) in waiters.iter().enumerate() {
                    if *thread_id == thread::current().id() {
                        waiters.remove(idx).unwrap();
                        // The worker was still waiting--this must have been a timeout wakeup
                        let WaitDuration::Timed(t) = self.duration else {
                            panic!("internal Fizzle error: mutex awakened despite thread still being in queue");
                        };

                        log::debug!("futex wait timed out after {:?}", t);
                        return Outcome::Error(FutexError::TimedOut);
                    }
                }

                Outcome::Success(())
            }
        }
    }
}

pub struct FutexWakeEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
}

impl<'a> FutexWakeEvent<'a> {
    pub fn new(uaddr: &'a mut u32, val: u32) -> Self {
        Self { uaddr, val }
    }
}

impl Event for FutexWakeEvent<'_> {
    type Success = usize;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(queue) = state
            .local
            .futex_waiters
            .get_mut(&FutexPtr::from_mut(self.uaddr))
        else {
            return Outcome::Success(0);
        };

        let mut awoken_threads = Vec::new();
        for _ in 0..self.val {
            match queue.pop_front() {
                Some(thread) => awoken_threads.push(thread),
                None => break,
            }
        }

        let ret = awoken_threads.len();

        for (_, thread) in awoken_threads {
            state.mark_thread_ready(thread);
        }

        Outcome::Success(ret)
    }
}

pub struct FutexRequeueEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
    timeout: Option<libc::timespec>,
    uaddr2: &'a mut u32,
}

impl<'a> FutexRequeueEvent<'a> {
    pub fn new(
        uaddr: &'a mut u32,
        val: u32,
        timeout: Option<libc::timespec>,
        uaddr2: &'a mut u32,
    ) -> Self {
        Self {
            uaddr,
            val,
            timeout,
            uaddr2,
        }
    }
}

impl Event for FutexRequeueEvent<'_> {
    type Success = usize;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(mut queue) = state
            .local
            .futex_waiters
            .remove(&FutexPtr::from_mut(self.uaddr))
        else {
            return Outcome::Success(0);
        };

        let mut awoken_threads = Vec::new();
        for _ in 0..self.val {
            match queue.pop_front() {
                Some(thread) => awoken_threads.push(thread),
                None => break,
            }
        }

        let ret = awoken_threads.len() + queue.len();

        match state
            .local
            .futex_waiters
            .entry(FutexPtr::from_mut(self.uaddr2))
        {
            Entry::Occupied(mut o) => o.get_mut().extend(queue),
            Entry::Vacant(v) => {
                v.insert(queue);
            }
        }

        Outcome::Success(ret)
    }
}

pub struct FutexCmpRequeueEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
    timeout: Option<libc::timespec>,
    uaddr2: &'a mut u32,
    val3: u32,
}

impl<'a> FutexCmpRequeueEvent<'a> {
    pub fn new(
        uaddr: &'a mut u32,
        val: u32,
        timeout: Option<libc::timespec>,
        uaddr2: &'a mut u32,
        val3: u32,
    ) -> Self {
        Self {
            uaddr,
            val,
            timeout,
            uaddr2,
            val3,
        }
    }
}

impl Event for FutexCmpRequeueEvent<'_> {
    type Success = usize;
    type Error = FutexError;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        if *self.uaddr != self.val3 {
            return Outcome::Error(FutexError::NotReady);
        }

        let Some(mut queue) = state
            .local
            .futex_waiters
            .remove(&FutexPtr::from_mut(self.uaddr))
        else {
            return Outcome::Success(0);
        };

        let mut awakened = Vec::new();
        for _ in 0..self.val {
            match queue.pop_front() {
                Some(thread) => awakened.push(thread),
                None => break,
            }
        }

        let ret = awakened.len() + queue.len();

        match state
            .local
            .futex_waiters
            .entry(FutexPtr::from_mut(self.uaddr2))
        {
            Entry::Occupied(mut o) => o.get_mut().extend(queue),
            Entry::Vacant(v) => {
                v.insert(queue);
            }
        }

        Outcome::Success(ret)
    }
}

pub struct FutexWakeOpEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
    uaddr2: &'a mut u32,
    val2: u32,
    val3: u32,
}

impl<'a> FutexWakeOpEvent<'a> {
    pub fn new(uaddr: &'a mut u32, val: u32, val2: u32, uaddr2: &'a mut u32, val3: u32) -> Self {
        Self {
            uaddr,
            val,
            val2,
            uaddr2,
            val3,
        }
    }
}

impl Event for FutexWakeOpEvent<'_> {
    type Success = usize;
    type Error = FutexError;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let oldval = *self.uaddr2;

        let op = self.val3 >> 28;
        let oparg = (self.val3 >> 12) & 0b1111_1111_1111;

        match op as i32 {
            libc::FUTEX_OP_SET => *self.uaddr2 = oparg,
            libc::FUTEX_OP_ADD => *self.uaddr2 += oparg,
            libc::FUTEX_OP_OR => *self.uaddr2 |= oparg,
            libc::FUTEX_OP_ANDN => *self.uaddr2 &= !oparg,
            libc::FUTEX_OP_XOR => *self.uaddr2 ^= oparg,
            FUTEX_OP_SHIFT_SET => *self.uaddr2 = 1 << oparg,
            FUTEX_OP_SHIFT_ADD => *self.uaddr2 += 1 << oparg,
            FUTEX_OP_SHIFT_OR => *self.uaddr2 |= 1 << oparg,
            FUTEX_OP_SHIFT_NAND => *self.uaddr2 &= !(1 << oparg),
            FUTEX_OP_SHIFT_XOR => *self.uaddr2 ^= 1 << oparg,
            5..=7 | 13..=15 => return Outcome::Error(FutexError::InvalidValue),
            _ => unreachable!(),
        }

        let awakened_1 = if let Some(queue) = state
            .local
            .futex_waiters
            .get_mut(&FutexPtr::from_mut(self.uaddr))
        {
            let mut awakened = Vec::new();
            for _ in 0..self.val {
                match queue.pop_front() {
                    Some(thread) => awakened.push(thread),
                    None => break,
                }
            }

            let ret = awakened.len();

            for (_, thread) in awakened {
                state.mark_thread_ready(thread);
            }

            ret
        } else {
            0
        };

        let cmp = (self.val3 >> 24) & 0b1111;
        let cmparg = self.val3 & 0b1111_1111_1111;

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
            let awakened_2 = if let Some(queue) = state
                .local
                .futex_waiters
                .get_mut(&&FutexPtr::from_mut(self.uaddr2))
            {
                let mut awoken_threads = Vec::new();
                for _ in 0..self.val2 {
                    match queue.pop_front() {
                        Some(thread) => awoken_threads.push(thread),
                        None => break,
                    }
                }

                let ret = awoken_threads.len();

                for (_, thread) in awoken_threads {
                    state.mark_thread_ready(thread);
                }

                ret
            } else {
                0
            };

            Outcome::Success(awakened_1 + awakened_2)
        } else {
            Outcome::Success(awakened_1)
        }
    }
}

pub struct FutexWaitBitsetEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
    duration: WaitDuration,
    state: FutexWaitState,
    val3: u32,
}

impl<'a> FutexWaitBitsetEvent<'a> {
    pub fn new(uaddr: &'a mut u32, val: u32, duration: WaitDuration, val3: u32) -> Self {
        Self {
            uaddr,
            val,
            duration,
            val3,
            state: FutexWaitState::Start,
        }
    }
}

impl Event for FutexWaitBitsetEvent<'_> {
    type Success = ();
    type Error = FutexError;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let futex_ptr = FutexPtr::from_mut(self.uaddr);

        match self.state {
            FutexWaitState::Start => {
                self.state = FutexWaitState::Finish;

                if *self.uaddr != self.val {
                    return Outcome::Error(FutexError::NotReady);
                }

                match state.local.futex_waiters.entry(futex_ptr) {
                    Entry::Occupied(mut o) => {
                        o.get_mut().push_back((self.val3, thread::current().id()))
                    }
                    Entry::Vacant(v) => {
                        let mut deque = VecDeque::new();
                        deque.push_back((self.val3, thread::current().id()));
                        v.insert(deque);
                    }
                };

                // Now wait for futex to be unblocked
                match self.duration {
                    WaitDuration::Immediate => unreachable!(), // No such thing as try* in futex semantics
                    WaitDuration::Indefinite => Outcome::Yield(YieldUntil::None),
                    WaitDuration::Timed(duration) => Outcome::Yield(YieldUntil::Reschedule(duration)),
                }
            }
            FutexWaitState::Finish => {
                let Some(waiters) = state.local.futex_waiters.get_mut(&futex_ptr) else {
                    panic!("internal Fizzle error: mutex destroyed while being waited on");
                };

                for (idx, (_, thread_id)) in waiters.iter().enumerate() {
                    if *thread_id == thread::current().id() {
                        waiters.remove(idx).unwrap();

                        // The worker was still waiting--this must have been a timeout wakeup
                        let WaitDuration::Timed(t) = self.duration else {
                            panic!("internal Fizzle error: mutex awakened despite thread still being in queue");
                        };

                        log::debug!("futex bitset wait timed out after {:?}", t);
                        return Outcome::Error(FutexError::TimedOut);
                    }
                }

                Outcome::Success(())
            }
        }
    }
}

pub struct FutexWakeBitsetEvent<'a> {
    uaddr: &'a mut u32,
    val: u32,
    val3: u32,
}

impl<'a> FutexWakeBitsetEvent<'a> {
    pub fn new(uaddr: &'a mut u32, val: u32, val3: u32) -> Self {
        Self { uaddr, val, val3 }
    }
}

impl Event for FutexWakeBitsetEvent<'_> {
    type Success = usize;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let Some(queue) = state
            .local
            .futex_waiters
            .get_mut(&FutexPtr::from_mut(self.uaddr))
        else {
            return Outcome::Success(0);
        };

        let mut awakened = Vec::new();

        for _ in 0..queue.len() {
            match queue.pop_front() {
                Some((bitmap, thread)) if (bitmap & self.val3) != 0 => awakened.push(thread),
                Some(entry) => queue.push_back(entry),
                None => break,
            }
        }

        let ret = awakened.len();

        if queue.is_empty() {
            state
                .local
                .futex_waiters
                .remove(&FutexPtr::from_mut(self.uaddr));
        }

        for thread in awakened {
            state.mark_thread_ready(thread);
        }

        Outcome::Success(ret)
    }
}
