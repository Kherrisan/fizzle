use rand_chacha::rand_core::TryRngCore;

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

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: this is a naive approach--make better later

        state.global.prefuzz_rng.try_fill_bytes(self.buf).unwrap();
        Outcome::Success(self.buf.len())
    }
}
