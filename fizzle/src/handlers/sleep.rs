use std::time::Duration;

use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

pub enum SleepState {
    Start,
    Finish,
}

pub struct SleepEvent {
    duration: Duration,
    state: SleepState,
}

impl SleepEvent {
    pub fn new(duration: Duration) -> Self {
        Self { duration, state: SleepState::Start }
    }
}

impl Event for SleepEvent {
    type Success = ();
    type Error = ();

    fn run(
        &mut self,
        _state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {

        match self.state {
            SleepState::Start => {
                // TODO: make all of these configurable
                if self.duration <= Duration::from_secs(1) {
                    self.state = SleepState::Finish;
                    Outcome::Retry
                } else if self.duration <= Duration::from_secs(10) {
                    self.state = SleepState::Finish;
                    Outcome::DelayedRetry
                } else {
                    Outcome::Continue
                }
            },
            SleepState::Finish => Outcome::Success(()),
        }
    }
}
