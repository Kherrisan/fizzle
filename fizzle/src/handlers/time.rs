use std::time::Duration;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::{FizzleState, ReadyInfo, ScheduledItem, TimerType};

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
        };

        let interval = match timer_info {
            Some(info) => info.interval,
            None => Duration::ZERO,
        };

        let current_pid = state.local.process_info.borrow().pid;

        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, ty) if &current_pid == pid && &self.which == ty => true,
            _ => false,
        });

        let val = match ready {
            Some(ScheduledItem { timestamp, .. }) => current_time.saturating_sub(*timestamp),
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
        };

        let old_interval = match timer_info {
            Some(info) => info.interval,
            None => Duration::ZERO,
        };

        let current_pid = state.local.process_info.borrow().pid;

        // See if there's already a scheduled timer
        let ready = state.global.ready.iter().find(|r| match &r.info {
            ReadyInfo::Timer(pid, ty) if &current_pid == pid && &self.which == ty => true,
            _ => false,
        });

        let old_remaining = match ready {
            Some(ScheduledItem { timestamp, .. }) => timestamp.saturating_sub(current_time),
            None => Duration::ZERO,
        };

        if let Some(ItimerValue { interval, val }) = &self.new_value {
            // If any timer was in the process of completing, remove it
            state.global.ready.retain(|r| match &r.info {
                ReadyInfo::Timer(pid, ty) if &current_pid == pid && &self.which == ty => false,
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
            }

            let timer_duration = if val.is_zero() { *interval } else { *val };

            if !timer_duration.is_zero() {
                // Add the new timer
                state.global.ready.push(ScheduledItem {
                    info: ReadyInfo::Timer(current_pid, self.which),
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

pub struct TimerPosixInfo {
    pub interval: Duration,
    pub signal: i32,  // TODO Store the signal number here
    pub exptime: Duration,
}

pub struct SigEvent {
    pub sigev_notify: i32,
    pub sigev_signo: i32,
    pub sigev_value: i32,
    pub sigev_notify_function: *mut libc::c_void,
    pub sigev_notify_attributes: *mut libc::c_void
}

pub struct TimerCreateEvent {
    pub clockid: libc::clockid_t,
    pub rvp: SigEvent,
    pub timerid: libc::timer_t
}
