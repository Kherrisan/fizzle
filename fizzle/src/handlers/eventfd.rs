use std::{cmp, os::fd::RawFd};

use crate::{
    arena::{ArenaKey, Rc}, scheduler::{Event, Outcome}, state::{FizzleSingleton, FizzleState}
};

pub use private::EventfdId;

use super::{
    descriptor::{DescriptorError, DescriptorId, DescriptorInfo, FdResource}, init_from_slice, polled::{PolledId, PolledInfo}, FfiOutput, MsgHdr, MsgHdrOut,
};

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct EventfdId(usize);
}

#[derive(Clone, Debug)]
pub struct EventfdInfo {
    pub read_polled: Rc<PolledId>,
    pub write_polled: Rc<PolledId>,
    pub is_semaphore: bool,
    pub counter: u64,
}

impl ArenaKey for EventfdId {
    type Value = EventfdInfo;
}

impl EventfdId {
    pub fn write(
        &self,
        ctx: &mut FizzleSingleton,
        msg: &impl MsgHdr,
        nonblocking: bool,
    ) -> Result<usize, EventfdError> {
        let mut event_val_bytes = [0u8; 8];
        let mut event_val_idx = 0;
        for iovec in msg.vdata() {
            let read_amount = cmp::min(8 - event_val_idx, iovec.data().len());
            event_val_bytes[event_val_idx..event_val_idx + read_amount]
                .copy_from_slice(&iovec.data()[..read_amount]);
            event_val_idx += read_amount;
        }

        if event_val_idx != 8 {
            return Err(EventfdError::InsufficientData);
        }

        let increment = u64::from_ne_bytes(event_val_bytes); // TODO: is this correct byte order?

        if increment == u64::MAX {
            return Err(EventfdError::InvalidWriteValue);
        }

        let state = ctx.acquire();

        let eventfd = state.global.event_fds.get(self).unwrap();
        let mut current_counter = eventfd.counter;
        let read_polled = eventfd.read_polled.clone();
        let write_polled = eventfd.write_polled.clone();

        drop(state);

        if nonblocking && current_counter.checked_add(increment + 1).is_none() {
            return Err(EventfdError::WouldBlock);
        }

        // The following code is designed very specifically to handle polling for arbitrary
        // `increment` values. Specifically, an application may choose to increment an
        // eventfd by up to `u64::MAX - 1`; however, if such an increment would cause the
        // eventfd to exceed its maximum permittable value (which is also `u64::MAX - 1`),
        // then the write operation for that increment should block until it can succeed.
        // This is the challenge: how do we know when to raise/lower a poll for a variably
        // chosen increment value so that writes preceding or following a blocked write will
        // still succeed if they are of a sufficiently small increment value?a
        //
        // The solution is as follows: check initially to see if the write will succeed.
        // Note that this DOES NOT use `polled_is_ready()`, but rather directly checks the
        // counter value added with the increment. If this would overflow the maximum value,
        // lower `write_polled` and poll until it has been raised again. Then check again;
        // continue this loop until succeeded.
        //
        // In the event that a large write blocks, smaller writes that would not overflow
        // the eventfd will still succeed, as this directly checks the addition value rather
        // than `polled_is_ready()` (`write_polled` will be lowered while a large write is
        // blocked). Whenever a read is performed, `write_polled` will be raised, triggering
        // an event for every poller waiting to write to the eventfd. This ensures that a
        // blocked writer will not remain blocked if the eventfd value drops low enough for
        // the read to succeed. If the performed read does not drop the value low enough, or
        // if another blocked write is carried out in between the read and the blocked write
        // check, the writer will simply loop again and lower/re-poll `write_polled` so that
        // the next subsequent read will trigger a notification. This solution is a bit
        // "noisy", in that every read awakes all blocked writers to check for readiness
        // instead of one, but it's what I could come up with within the constraints of
        // Fizzle's current polling infrastructure.
        //
        // As a happy side note, the combination of this algorithm with the LIFO data
        // structures we use for holding pollers within `polled` instances means that a
        // blocked write is guaranteed to always eventually be at the top of the queue
        // following each read, thereby ensuring no blocked write is starved.
        let new_counter = loop {
            match current_counter.checked_add(increment) {
                Some(c) if c != u64::MAX => break c,
                _ => {
                    let mut state = ctx.acquire();
                    state.lower_polled(&write_polled);
                    drop(state);

                    ctx.poll_until_ready(write_polled.clone());

                    let state = ctx.acquire();
                    current_counter = state.global.event_fds.get(self).unwrap().counter;
                    drop(state);
                }
            }
        };

        let mut state = ctx.acquire();
        state.global.event_fds.get_mut(self).unwrap().counter = new_counter;
        state.raise_polled(&read_polled);
        if new_counter == u64::MAX - 1 {
            state.lower_polled(&write_polled);
        }

        Ok(8) // An eventfd always reads exactly 8 bytes from the buffer.
    }

    pub fn read(
        &self,
        ctx: &mut FizzleSingleton,
        msg: &mut MsgHdrOut,
        nonblocking: bool,
    ) -> Result<usize, EventfdError> {
        let state = ctx.acquire();

        let eventfd = state.global.event_fds.get(self).unwrap();
        let is_semaphore = eventfd.is_semaphore;
        let old_counter = eventfd.counter;
        let read_polled = eventfd.read_polled.clone();
        let write_polled = eventfd.write_polled.clone();

        drop(state);

        if old_counter == 0 {
            if nonblocking {
                return Err(EventfdError::WouldBlock);
            } else {
                ctx.poll_until_ready(read_polled.clone());
            }
        }

        let mut state = ctx.acquire();
        let eventfd = state.global.event_fds.get_mut(self).unwrap();

        let ret: u64 = match is_semaphore {
            true => 1,
            false => eventfd.counter,
        };

        let event_val_bytes = ret.to_ne_bytes();
        let mut event_val_idx = 0;

        for iovec in msg.vdata_mut() {
            let write_amount = cmp::min(8 - event_val_idx, iovec.data_mut().len());
            init_from_slice(
                iovec.data_mut(),
                &event_val_bytes[event_val_idx..event_val_idx + write_amount],
            );
            event_val_idx += write_amount;
        }

        if event_val_idx != 8 {
            return Err(EventfdError::InsufficientData);
        }

        if is_semaphore {
            eventfd.counter -= 1;
        } else {
            eventfd.counter = 0;
        }

        if eventfd.counter == 0 {
            state.lower_polled(&read_polled);
        }
        state.raise_polled(&write_polled);

        Ok(8) // An eventfd always writes exactly 8 bytes to the buffer.
    }
}

pub enum EventfdError {
    /// A socket-specific operation was attempted on the eventfd (such as `send()`).
    NotSocket,
    /// Not enough bytes were available in the supplied buffer to read/write an eventfd value.
    InsufficientData,
    /// The invalid value 0xffffffff was used in a call to `write()`.
    InvalidWriteValue,
    /// The read or write would cause the eventfd to block, and the eventfd is nonblocking.
    WouldBlock,
}

impl From<EventfdError> for DescriptorError {
    fn from(value: EventfdError) -> Self {
        match value {
            EventfdError::NotSocket => Self::NotSocket,
            EventfdError::InsufficientData => Self::InvalidInput,
            EventfdError::InvalidWriteValue => Self::InvalidInput,
            EventfdError::WouldBlock => Self::WouldBlock,
        }
    }
}

impl FfiOutput for Result<usize, EventfdError> {
    type OutputType = libc::c_int;

    fn out(&self) -> Self::OutputType {
        match self {
            Ok(i) => {
                Self::set_errno(0);
                return *i as i32;
            }
            Err(EventfdError::NotSocket) => Self::set_errno(libc::ENOTSOCK),
            Err(EventfdError::InsufficientData) => Self::set_errno(libc::EINVAL),
            Err(EventfdError::InvalidWriteValue) => Self::set_errno(libc::EINVAL),
            Err(EventfdError::WouldBlock) => Self::set_errno(libc::EAGAIN),
        }

        -1
    }

    fn display(&self) -> &'static str {
        match self {
            Ok(_) => "8",
            Err(EventfdError::NotSocket) => "-1 (ENOTSOCK)",
            Err(EventfdError::InsufficientData) => "-1 (EINVAL)",
            Err(EventfdError::InvalidWriteValue) => "-1 (EINVAL)",
            Err(EventfdError::WouldBlock) => "-1 (EAGAIN)",
        }
    }
}

pub struct EventfdCreateEvent {
    initial_value: libc::c_uint,
    is_semaphore: bool,
    close_on_exec: bool,
    nonblocking: bool,
    
}

impl EventfdCreateEvent {
    pub fn new(initial_value: libc::c_uint, is_semaphore: bool, close_on_exec: bool, nonblocking: bool) -> Self {
        Self { initial_value, is_semaphore, close_on_exec, nonblocking }
    }
}

impl Event for EventfdCreateEvent {
    type Success = RawFd;
    type Error = ();

    fn run(
        &mut self,
        state: &mut FizzleState,
    ) -> Outcome<Self::Success, Self::Error> {
        let fd = crate::create_descriptor();

        let read_polled = state.global.polled_events.allocate(if self.initial_value == 0 { PolledInfo::new() } else { PolledInfo::new_raised() }).unwrap();
        let write_polled = state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap();

        let eventfd_id = state.global.event_fds.allocate(EventfdInfo {
            read_polled,
            write_polled,
            is_semaphore: self.is_semaphore,
            counter: self.initial_value as u64,
        }).unwrap();

        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
            close_on_exec: self.close_on_exec,
            nonblocking: self.nonblocking,
            is_passthrough: false,
            resource: FdResource::EventFd(eventfd_id),
        }).unwrap();

        Outcome::Success(fd)
    }
}
