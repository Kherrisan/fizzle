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
        Self {
            duration,
            state: SleepState::Start,
        }
    }
}

impl Event for SleepEvent {
    type Success = ();
    type Error = ();

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match self.state {
            SleepState::Start => Outcome::Yield(Some(self.duration)),
            SleepState::Finish => Outcome::Success(()),
        }
    }
}
