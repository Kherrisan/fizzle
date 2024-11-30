use std::thread;
use std::time::Duration;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

pub enum SleepState {
    Start,
    Finish,
}

pub struct SleepEvent {
    duration: Option<Duration>,
    state: SleepState,
}

impl SleepEvent {
    pub fn new(duration: Option<Duration>) -> Self {
        Self {
            duration,
            state: SleepState::Start,
        }
    }
}

impl Event for SleepEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let thread_id = thread::current().id();

        match self.state {
            SleepState::Start => {
                self.state = SleepState::Finish;

                let signal_info = state.local.signals.get_mut(&thread_id).unwrap();
                signal_info.sigsuspend = true;
                signal_info.interrupted = false;
                Outcome::Yield(self.duration)
            }
            SleepState::Finish => {
                let signal_info = state.local.signals.get_mut(&thread_id).unwrap();
                signal_info.sigsuspend = false;

                if signal_info.interrupted {
                    Outcome::Error(Errno::EINTR)
                } else {
                    Outcome::Success(())
                }
            }
        }
    }
}
