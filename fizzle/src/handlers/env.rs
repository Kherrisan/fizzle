use std::ffi::CStr;

use crate::scheduler::{Event, Outcome};


pub struct GetEnvEvent<'a> {
    name: &'a CStr,
}

impl<'a> GetEnvEvent<'a> {
    pub fn new(name: &'a CStr) -> Self {
        Self {
            name,
        }
    }
}

impl Event for GetEnvEvent<'_> {
    type Success = *mut libc::c_char;
    type Error = ();
    
    fn run(&mut self, _state: &mut crate::state::FizzleState) -> crate::scheduler::Outcome<Self::Success, Self::Error> {
        Outcome::Success(unsafe { libc::getenv(self.name.as_ptr()) })
    }
}
