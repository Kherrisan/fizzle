use std::cell::RefCell;
use std::cmp;
use std::rc::{Rc, Weak};
use std::time::Duration;

use crate::errno::Errno;
use crate::GlobalRc;
use crate::handlers::descriptor::{Descriptor, DescriptorInfo, FdResource, ReadData};
use crate::handlers::poller::PollerInfo;
use crate::scheduler::{Event, fizzle_alloc, Outcome, YieldUntil};
use crate::state::{FizzleState, ReadyInfo, ScheduledItem, TimerType, TimerIdType};

use super::polled::PolledInfo;

#[derive(Clone)]
pub struct ItimerInfo {
    pub interval: Duration,
}

pub struct GetTimeEvent;

impl Event for GetTimeEvent {
    type Success = Duration;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        Outcome::Success(state.global.current_time)
    }
}

pub struct GetTimesEvent;

impl Event for GetTimesEvent {
    type Success = libc::tms;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let start = Duration::from_secs(1735924847);
        let current = state.global.current_time;
        let diff = current - start;

        // hardcoded 4GHz clock
        Outcome::Success(libc::tms {
            tms_utime: 4 * diff.as_nanos() as i64,
            tms_stime: 4 * diff.as_nanos() as i64,
            tms_cutime: 4 * diff.as_nanos() as i64,
            tms_cstime: 4 * diff.as_nanos() as i64,
        })
    }
}

pub struct ItimerValue {
    pub interval: Duration,
    pub val: Duration,
}

pub struct GetItimerEvent {
    which: TimerType,
}

impl GetItimerEvent {
    pub fn new(which: TimerType) -> Self {
        Self { which }
    }
}

impl Event for GetItimerEvent {
    type Success = ItimerValue;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_time = state.global.current_time;

        let timer_info = match &self.which {
            TimerType::Real => &state.local.itimer_real,
            TimerType::Virtual => &state.local.itimer_virtual,
            TimerType::Prof => &state.local.itimer_prof,
            TimerType::ClockRealtime => &state.local.itimer_real,
            TimerType::ClockMonotonic => &state.local.itimer_real,
        };

        let interval = match timer_info {
            Some(info) => info.interval,
            None => Duration::ZERO,
        };

        let current_pid = state.local.process_info.borrow().pid;

        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, ty, timerid, signo) if &current_pid == pid && &self.which == ty => true,
            _ => false,
        });

        let val = match ready {
            Some(ScheduledItem { timestamp, .. }) => (*timestamp).saturating_sub(current_time),
            None => Duration::ZERO,
        };

        /*
        state.global.ready.retain(|r| match &r.info {
            ReadyInfo::Timer(pid, ty) if &current_pid == pid && &self.which == ty => false,
            _ => true,
        });
        */

        Outcome::Success(ItimerValue { interval, val })
    }
}

pub struct SetItimerEvent {
    which: TimerType,
    new_value: Option<ItimerValue>,
}

impl SetItimerEvent {
    pub fn new(which: TimerType, new_value: Option<ItimerValue>) -> Self {
        Self { which, new_value }
    }
}

impl Event for SetItimerEvent {
    type Success = ItimerValue;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_time = state.global.current_time;

        let timer_info = match self.which {
            TimerType::Real => &state.local.itimer_real,
            TimerType::Virtual => &state.local.itimer_virtual,
            TimerType::Prof => &state.local.itimer_prof,
            TimerType::ClockRealtime => &state.local.itimer_real,
            TimerType::ClockMonotonic => &state.local.itimer_real,
        };

        let old_interval = match timer_info {
            Some(info) => info.interval,
            None => Duration::ZERO,
        };

        let current_pid = state.local.process_info.borrow().pid;

        // See if there's already a scheduled timer
        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, ty, timerid, signo) if &current_pid == pid && &self.which == ty => true,
            _ => false,
        });

        let old_remaining = match ready {
            Some(ScheduledItem { timestamp, .. }) => timestamp.saturating_sub(current_time),
            None => Duration::ZERO,
        };

        if let Some(ItimerValue { interval, val }) = &self.new_value {
            // If any timer was in the process of completing, remove it
            state.global.ready.retain(|r| match &r.info {
                ReadyInfo::Timer(pid, ty, timerid, signo) if &current_pid == pid && &self.which == ty => false,
                _ => true,
            });

            match self.which {
                TimerType::Real => {
                    state.local.itimer_real = if interval.is_zero() {
                        None
                    } else {
                        Some(ItimerInfo {
                            interval: *interval,
                        })
                    }
                }
                TimerType::Virtual => {
                    state.local.itimer_virtual = if interval.is_zero() {
                        None
                    } else {
                        Some(ItimerInfo {
                            interval: *interval,
                        })
                    }
                }
                TimerType::Prof => {
                    state.local.itimer_prof = if interval.is_zero() {
                        None
                    } else {
                        Some(ItimerInfo {
                            interval: *interval,
                        })
                    }
                }
                TimerType::ClockRealtime => {}
                TimerType::ClockMonotonic => {}
            }

            let timer_duration = if val.is_zero() { *interval } else { *val };

            if !timer_duration.is_zero() {
                // Add the new timer
                state.global.ready.push(ScheduledItem {
                    info: ReadyInfo::Timer(current_pid, self.which, TimerIdType::Itimer(()), None),
                    timestamp: current_time + timer_duration,
                });
            }
        };

        Outcome::Success(ItimerValue {
            interval: old_interval,
            val: old_remaining,
        })
    }
}

pub struct TimerPosixState {
    pub next_timer: i64,  // The next timer ID to assign
}

#[derive(Clone, Copy)]
pub struct TimerPosixInfo {
    pub clockid: libc::clockid_t,
    pub interval: Duration,
    pub signal: Option<i32>,
    pub exptime: Duration,
    /// Number of timer expirations since the last `timer_settime()` call
    pub overruns: libc::c_int,
}

/// Used to holder information about the timer file descriptor when
/// created by the `timerfd_*()` functions in Linux.
pub struct TimerfdInfo {
    pub polled: GlobalRc<PolledInfo>,
    /// Information about the timer itself
    pub timerid: i64,
}

pub struct TimerCreateEvent {
    pub clockid: libc::clockid_t,
    pub signal_to_send: Option<i32>,
}

impl TimerCreateEvent {
    pub fn new(clockid: libc::clockid_t, signal_to_send: Option<i32>) -> Self {
        Self { clockid, signal_to_send }
    }
}

impl Event for TimerCreateEvent {
    type Success = i64;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state.local.timer_posix_state.next_timer += 1;
        let current_timer_id = state.local.timer_posix_state.next_timer - 1;

        state.local.timers_posix.insert(state.local.timer_posix_state.next_timer - 1, TimerPosixInfo {
            clockid: self.clockid,  // Store the clock ID for later use
            interval: Duration::ZERO,
            signal: self.signal_to_send,
            exptime: Duration::ZERO,
            overruns: 0,  // No overruns yet, this is a fresh new timer.
        });

        // Grab the current process's PID and the current time for later use
        let current_pid = state.local.process_info.borrow().pid;
        let current_time = state.global.current_time;

        // Add the timer to the priority queue.
        /*
        state.global.ready.push(ScheduledItem {
            info: ReadyInfo::Timer(current_pid, self.which, current_timer_id, self.signal_to_send),
            timestamp: current_time + timer_duration,
        });
        */

        Outcome::Success(current_timer_id)
    }
}

pub struct TimerDeleteEvent {
    pub timerid: i64,
}

impl TimerDeleteEvent {
    pub fn new(timerid: i64) -> Self {
        Self { timerid }
    }
}

impl Event for TimerDeleteEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // Technically it's supposed to return EINVAL on Linux, but it's undefined
        // in the POSIX standards so we'll ignore wheteher it was success or
        // failure unless something breaks.
        
        let current_pid = state.local.process_info.borrow().pid;

        state.global.ready.retain(|r| match &r.info {
            ReadyInfo::Timer(pid, type_, timerid, signo) if 
                &current_pid == pid && &TimerIdType::Timer(self.timerid) == timerid => true,
            _ => false,
        });

        Outcome::Success(())
   }
}

pub struct TimerGettimeEvent {
    pub timerid: i64,
}

impl TimerGettimeEvent {
    pub fn new(timerid: i64) -> Self {
        Self { timerid }
    }
}

impl Event for TimerGettimeEvent {
    type Success = ItimerValue;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_time = state.global.current_time;

        /// The TimerInfo object currently stored in the local state
        let Some(timer_info_const) = state.local.timers_posix.get(&self.timerid) else { return Outcome::Error(Errno::EINVAL); };

        let current_pid = state.local.process_info.borrow().pid;

        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, type_, timerid, signo) 
                if &current_pid == pid && &TimerIdType::Timer(self.timerid) == timerid => true,
            _ => false,
        });

        let timer_interval = timer_info_const.interval;

        let timer_value = match ready {
            Some(ScheduledItem { timestamp, .. }) => (*timestamp).saturating_sub(current_time),
            None => Duration::ZERO,
        };

        Outcome::Success(ItimerValue {
            interval: timer_interval,
            val: timer_value,
        })
    }
}

pub struct TimerSettimeEvent {
    pub timerid: i64,
    pub is_absolute: bool,
    pub new_value: ItimerValue,
}

impl TimerSettimeEvent {
    pub fn new(timerid: i64, is_absolute: bool, new_value: ItimerValue) -> Self {
        Self { timerid, is_absolute, new_value }
    }
}

impl Event for TimerSettimeEvent {
    type Success = ItimerValue;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_time = state.global.current_time;

        /// The TimerInfo object currently stored in the local state
        let Some(timer_info_const) = state.local.timers_posix.get(&self.timerid) else { return Outcome::Error(Errno::EINVAL); };

        let mut timer_info: TimerPosixInfo = TimerPosixInfo {
            clockid: timer_info_const.clockid,
            interval: timer_info_const.interval,
            signal: timer_info_const.signal,
            exptime: timer_info_const.exptime,
            overruns: timer_info_const.overruns
        };

        let old_interval = timer_info.interval;

        let current_pid = state.local.process_info.borrow().pid;

        // See if there's already a scheduled timer
        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, type_, timerid, signo) if 
                &current_pid == pid && &TimerIdType::Timer(self.timerid) == timerid => true,
            _ => false,
        });

        let old_remaining = match ready {
            Some(ScheduledItem { timestamp, .. }) => timestamp.saturating_sub(current_time),
            None => Duration::ZERO,
        };

        if let ItimerValue { interval, val } = &self.new_value {
            // If any timer was in the process of completing, remove it
            state.global.ready.retain(|r| match &r.info {
                ReadyInfo::Timer(pid, type_, timerid, signo) if 
                    &current_pid == pid && &TimerIdType::Timer(self.timerid) == timerid => true,
                _ => false,
            });

            // Set the interval and expiration time for the TimerInfo object
            timer_info.interval = self.new_value.interval;
            if (self.is_absolute) {
                timer_info.exptime = self.new_value.val - current_time;
            }
            else {
                timer_info.exptime = self.new_value.val;
            }

            // Put the new and improved TimerPosixInfo object back into the HashMap
            state.local.timers_posix.insert(self.timerid, timer_info);

            let timer_duration = if timer_info.interval.is_zero() { timer_info.interval } else { timer_info.exptime };

            if !timer_duration.is_zero() {
                // Add the new timer
                state.global.ready.push(ScheduledItem {
                    info: ReadyInfo::Timer(current_pid, TimerType::ClockRealtime, 
                              TimerIdType::Timer(self.timerid), timer_info.signal),
                    timestamp: current_time + timer_duration,
                });
            }
        };

        Outcome::Success(ItimerValue {
            interval: old_interval,
            val: old_remaining,
        })
    }
}


pub enum TimerfdReadState {
    Start,
    Finish(Option<GlobalRc<PollerInfo>>),
}

pub struct TimerfdReadEvent<'a> {
    timerfd_info: GlobalRc<TimerfdInfo>,
    nonblocking: bool,
    data: ReadData<'a>,
    state: TimerfdReadState,
}

impl<'a> TimerfdReadEvent<'a> {
    #[inline]
    pub fn new(timerfd_info: GlobalRc<TimerfdInfo>, nonblocking: bool, data: ReadData<'a>) -> Self {
        Self {
            timerfd_info,
            nonblocking,
            data,
            state: TimerfdReadState::Start,
        }
    }
}

impl Event for TimerfdReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let ReadData::Iovec(iovec) = &mut self.data else {
            unreachable!(
                "internal error--buffer other than ReadData::Iovec passed to TimerfdReadEvent"
            );
        };

        let timer_id = self.timerfd_info.borrow().timerid;

        match &self.state {
            TimerfdReadState::Start => {
                let polled = self.timerfd_info.borrow().polled.clone();

                if state.polled_is_ready(&polled) {
                    self.state = TimerfdReadState::Finish(None);
                    Outcome::Yield(YieldUntil::Immediate)
                } else if self.nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), polled.clone());
                    self.state = TimerfdReadState::Finish(Some(poller_id));
                    Outcome::Yield(YieldUntil::None)
                }
            }
            TimerfdReadState::Finish(poller) => {
                if let Some(poller) = poller {
                    state.delete_poller(poller.clone());
                }
                let Some(timer_info) = state.local.timers_posix.get(&timer_id) 
                    else { panic!("Invalid timer ID stored in FdResource::Timerfd(TimerFdInfo)") };
                // Read the value out of the variable
                // TODO Get the correct value out of the state
                let overruns: u64 = timer_info.overruns as u64;

                // Convert overruns integer into bytes in platform-native
                // order.
                let overruns_bytes = overruns.to_ne_bytes();

                let mut total_read = 0;
                for slice in iovec.iter_mut() {
                    let v_read = cmp::min(overruns_bytes.len() - total_read, slice.len());
                    slice.copy_from_slice(&overruns_bytes[total_read..total_read + v_read]);
                    total_read += v_read;
                }

                Outcome::Success(total_read)
            }
        }
    }
}


pub struct TimerfdCreateEvent {
    /// The file descriptor number with which to create this `Timerfd` object
    pub fd: i32,
    pub clockid: libc::clockid_t,
}

impl TimerfdCreateEvent {
    pub fn new(fd: i32, clockid: libc::clockid_t) -> Self {
        Self { fd, clockid }
    }
}

impl Event for TimerfdCreateEvent {
    type Success = i64;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        state.local.timer_posix_state.next_timer += 1;
        let current_timer_id = state.local.timer_posix_state.next_timer - 1;

        state.local.timers_posix.insert(current_timer_id, TimerPosixInfo {
            clockid: self.clockid,
            interval: Duration::ZERO,
            signal: None,
            exptime: Duration::ZERO,
            overruns: 0,  // No overruns yet, this is a new timer.
        });

        state.local.fds.insert(
            Descriptor::from_raw_fd(self.fd),
            DescriptorInfo {
                close_on_exec: false,
                nonblocking: false,
                is_passthrough: false,
                is_random: false,
                // Rc - heap allocated object with reference counter ("smart pointer").
                // fizzle_alloc() - The custom allocator for Fizzle that allocates into shared
                // memory across all processes.
                // RefCell - "global variable", allows mutability to be checked at runtime rather
                // than compile time.
                // PolledInfo - You can call `raise_polled()` to indicate that the polling
                // processes should be woken up.
                resource: FdResource::Timerfd(Rc::new_in(
                        RefCell::new(TimerfdInfo {
                            polled: Rc::new_in(
                                RefCell::new(PolledInfo {
                                    pollers: Vec::new_in(fizzle_alloc()),
                                    event_raised: false,
                                }),
                                fizzle_alloc(),
                            ),
                            timerid: current_timer_id,
                    }),
                    fizzle_alloc(),
                ))
            }
        );

        Outcome::Success(current_timer_id)
    }
}


pub struct TimerfdSettimeEvent {
    pub fd: libc::c_int,
    pub is_absolute: bool,
    pub new_value: ItimerValue,
}

impl TimerfdSettimeEvent {
    pub fn new(fd: libc::c_int, is_absolute: bool, new_value: ItimerValue) -> Self {
        Self { fd, is_absolute, new_value }
    }
}

impl Event for TimerfdSettimeEvent {
    type Success = ItimerValue;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let current_time = state.global.current_time;

        let Some(fd_info) = state.local.fds.get(&Descriptor::from_raw_fd(self.fd)) else {
            return Outcome::Error(Errno::EBADF);
        };

        let timerfd_info_const = match &fd_info.resource {
            FdResource::Timerfd(timerfd_info_const) => timerfd_info_const,
            _ => return Outcome::Error(Errno::EINVAL),
        };

        /// The internal ID given to this timer.
        let internal_timer_id = timerfd_info_const.borrow().timerid;

        /// The TimerInfo object currently stored in the local state
        let Some(timer_info_const) = state.local.timers_posix.get(&internal_timer_id) else { panic!("Invalid timer ID stored in FdResource::Timerfd(TimerFdInfo)") };

        let mut timer_info: TimerPosixInfo = TimerPosixInfo {
            clockid: timer_info_const.clockid,
            interval: timer_info_const.interval,
            signal: timer_info_const.signal,
            exptime: timer_info_const.exptime,
            overruns: timer_info_const.overruns,
        };

        let old_interval = timer_info.interval;

        let current_pid = state.local.process_info.borrow().pid;

        // See if there's already a scheduled timer
        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, type_, timerfdint, signo) if 
                &current_pid == pid && &TimerIdType::Fd(self.fd) == timerfdint => true,
            _ => false,
        });

        let old_remaining = match ready {
            Some(ScheduledItem { timestamp, .. }) => timestamp.saturating_sub(current_time),
            None => Duration::ZERO,
        };

        if let ItimerValue { interval, val } = &self.new_value {
            // If any timer was in the process of completing, remove it
            state.global.ready.retain(|r| match &r.info {
                ReadyInfo::Timer(pid, type_, timerfdint, signo) if 
                    &current_pid == pid && &TimerIdType::Fd(self.fd) == timerfdint => true,
                _ => false,
            });

            // Set the interval and expiration time for the TimerInfo object
            timer_info.interval = self.new_value.interval;
            if (self.is_absolute) {
                timer_info.exptime = self.new_value.val - current_time;
            }
            else {
                timer_info.exptime = self.new_value.val;
            }

            // Put the new and improved TimerPosixInfo object back into the HashMap
            state.local.timers_posix.insert(internal_timer_id, timer_info);

            let timer_duration = if timer_info.interval.is_zero() { timer_info.interval } else { timer_info.exptime };

            if !timer_duration.is_zero() {
                // Add the new timer
                // TODO What to put instead of the ReadyInfo::Timer? We need to 
                // update the internal state some how so that, when the
                // timer expires, an overrun counter is incremented and 
                // any select() or poll() calls waiting on this timer
                // should be notified. Also, anyone blocked on read()
                // should get unblocked.
                state.global.ready.push(ScheduledItem {
                    info: ReadyInfo::Timer(current_pid, TimerType::ClockRealtime, TimerIdType::Fd(self.fd), timer_info.signal),
                    timestamp: current_time + timer_duration,
                });
            }
        };

        Outcome::Success(ItimerValue {
            interval: old_interval,
            val: old_remaining,
        })
    }
}
