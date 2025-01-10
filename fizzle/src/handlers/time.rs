use std::time::Duration;

use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

pub struct GetTimeEvent;

impl Event for GetTimeEvent {
    type Success = Duration;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        Outcome::Success(state.global.current_time)
    }
}
