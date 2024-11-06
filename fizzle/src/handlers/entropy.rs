
use rand::Fill;

use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

pub struct GetEntropyEvent<'a> {
    buf: &'a mut [u8],
}

impl<'a> GetEntropyEvent<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf }
    }
}

impl Event for GetEntropyEvent<'_> {
    type Success = usize;
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {
        // TODO: this is a naive approach--make better later

        if state.global.fuzz_input.is_empty() {
            // Fuzzing round hasn't started yet--seed with fundamental RNG source

            // This *should* always fill properly
            self.buf.try_fill(&mut state.global.prefuzz_rng).unwrap();
            Outcome::Success(self.buf.len())

        } else {
            // Fill directly with fuzzing input

            let mut idx = 0usize;
            let fuzz_len = state.global.fuzz_input.len();
            while self.buf[idx..].len() > state.global.fuzz_input.len() {
                self.buf[idx..idx + fuzz_len].copy_from_slice(state.global.fuzz_input.data());
                idx += fuzz_len;
            }
            
            let rem = self.buf.len() - idx;
            if rem > 0 {
                self.buf[idx..].copy_from_slice(&state.global.fuzz_input.data()[..rem]);
            }

            Outcome::Success(self.buf.len())
        }
    }
}