use std::{cmp, os::fd::RawFd};

use crate::arena::{ArenaKey, Rc};
use crate::errno::Errno;
use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

pub use private::EventfdId;

use super::polled::{PolledId, PolledInfo};
use super::{descriptor::*, poller::PollerId};

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

pub struct EventfdCreateEvent {
    initial_value: libc::c_uint,
    is_semaphore: bool,
    close_on_exec: bool,
    nonblocking: bool,
}

impl EventfdCreateEvent {
    pub fn new(
        initial_value: libc::c_uint,
        is_semaphore: bool,
        close_on_exec: bool,
        nonblocking: bool,
    ) -> Self {
        Self {
            initial_value,
            is_semaphore,
            close_on_exec,
            nonblocking,
        }
    }
}

impl Event for EventfdCreateEvent {
    type Success = RawFd;
    type Error = ();

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let fd = crate::create_descriptor();

        let read_polled = state
            .global
            .polled_events
            .allocate(if self.initial_value == 0 {
                PolledInfo::new()
            } else {
                PolledInfo::new_raised()
            })
            .unwrap();
        let write_polled = state
            .global
            .polled_events
            .allocate(PolledInfo::new_raised())
            .unwrap();

        let eventfd_id = state
            .global
            .event_fds
            .allocate(EventfdInfo {
                read_polled,
                write_polled,
                is_semaphore: self.is_semaphore,
                counter: self.initial_value as u64,
            })
            .unwrap();

        state
            .local
            .fds
            .allocate_with_key(
                DescriptorId::from_raw_fd(fd),
                DescriptorInfo {
                    close_on_exec: self.close_on_exec,
                    nonblocking: self.nonblocking,
                    is_passthrough: false,
                    resource: FdResource::EventFd(eventfd_id),
                },
            )
            .unwrap();

        Outcome::Success(fd)
    }
}

pub enum EventfdReadState {
    Start,
    Finish(Option<Rc<PollerId>>),
}

pub struct EventfdReadEvent<'a> {
    eventfd_id: Rc<EventfdId>,
    nonblocking: bool,
    data: ReadData<'a>,
    state: EventfdReadState,
}

impl<'a> EventfdReadEvent<'a> {
    #[inline]
    pub fn new(eventfd_id: Rc<EventfdId>, nonblocking: bool, data: ReadData<'a>) -> Self {
        Self {
            eventfd_id,
            nonblocking,
            data,
            state: EventfdReadState::Start,
        }
    }
}

impl Event for EventfdReadEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let ReadData::Basic(iovec) = &mut self.data else {
            unreachable!(
                "internal error--buffer other than ReadData::Basic passed to EventfdReadEvent"
            );
        };

        match &self.state {
            EventfdReadState::Start => {
                let eventfd = state.global.event_fds.get(&self.eventfd_id).unwrap();
                let old_counter = eventfd.counter;
                let read_polled = eventfd.read_polled.clone();

                if old_counter > 0 {
                    self.state = EventfdReadState::Finish(None);
                    Outcome::Continue
                } else if self.nonblocking {
                    Outcome::Error(Errno::EAGAIN)
                } else {
                    let poller_id = state.new_poller();
                    state.register_poller(poller_id.clone(), read_polled);

                    self.state = EventfdReadState::Finish(Some(poller_id));
                    Outcome::Yield(None)
                }
            }
            EventfdReadState::Finish(poller_id) => {
                if let Some(poller_id) = poller_id {
                    state.delete_poller(poller_id.clone());
                }

                let eventfd = state.global.event_fds.get(&self.eventfd_id).unwrap();
                let is_semaphore = eventfd.is_semaphore;
                let read_polled = eventfd.read_polled.clone();
                let write_polled = eventfd.write_polled.clone();

                let eventfd = state.global.event_fds.get_mut(&self.eventfd_id).unwrap();

                let ret: u64 = match is_semaphore {
                    true => 1,
                    false => eventfd.counter,
                };

                let event_val_bytes = ret.to_ne_bytes();
                let mut event_val_idx = 0;

                for slice in iovec.iter_mut() {
                    let write_amount = cmp::min(8 - event_val_idx, slice.len());
                    slice.copy_from_slice(
                        &event_val_bytes[event_val_idx..event_val_idx + write_amount],
                    );
                    event_val_idx += write_amount;
                }

                if event_val_idx != 8 {
                    return Outcome::Error(Errno::EINVAL);
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

                Outcome::Success(8) // An eventfd always writes exactly 8 bytes to the buffer.
            }
        }
    }
}

pub enum EventfdWriteState {
    Start,
    Finish(Option<Rc<PollerId>>),
}

pub struct EventfdWriteEvent<'a> {
    eventfd_id: Rc<EventfdId>,
    nonblocking: bool,
    data: WriteData<'a>,
    state: EventfdWriteState,
}

impl<'a> EventfdWriteEvent<'a> {
    #[inline]
    pub fn new(eventfd_id: Rc<EventfdId>, nonblocking: bool, data: WriteData<'a>) -> Self {
        Self {
            eventfd_id,
            nonblocking,
            data,
            state: EventfdWriteState::Start,
        }
    }
}

impl Event for EventfdWriteEvent<'_> {
    type Success = usize;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let WriteData::Basic(iovec) = &self.data else {
            unreachable!(
                "internal error--buffer other than WriteData::Basic passed to EventfdWriteEvent"
            );
        };

        match &self.state {
            EventfdWriteState::Start => {
                let mut event_val_bytes = [0u8; 8];
                let mut event_val_idx = 0;
                for slice in iovec.iter() {
                    let read_amount = cmp::min(8 - event_val_idx, slice.len());
                    event_val_bytes[event_val_idx..event_val_idx + read_amount]
                        .copy_from_slice(&slice[..read_amount]);
                    event_val_idx += read_amount;
                }

                if event_val_idx != 8 {
                    return Outcome::Error(Errno::EINVAL);
                }

                let increment = u64::from_ne_bytes(event_val_bytes);

                if increment == u64::MAX {
                    return Outcome::Error(Errno::EINVAL);
                }

                let eventfd = state.global.event_fds.get(&self.eventfd_id).unwrap();
                let current_counter = eventfd.counter;
                let write_polled = eventfd.write_polled.clone();

                if current_counter.checked_add(increment + 1).is_none() {
                    if self.nonblocking {
                        Outcome::Error(Errno::EAGAIN)
                    } else {
                        let poller_id = state.new_poller();
                        state.lower_polled(&write_polled);
                        state.register_poller(poller_id.clone(), write_polled);

                        self.state = EventfdWriteState::Finish(Some(poller_id));
                        Outcome::Yield(None)
                    }
                } else {
                    self.state = EventfdWriteState::Finish(None);
                    Outcome::Continue
                }
            }
            EventfdWriteState::Finish(poller_id) => {
                if let Some(poller_id) = poller_id {
                    state.delete_poller(poller_id.clone());
                }

                let mut event_val_bytes = [0u8; 8];
                let mut event_val_idx = 0;
                for slice in iovec.iter() {
                    let read_amount = cmp::min(8 - event_val_idx, slice.len());
                    event_val_bytes[event_val_idx..event_val_idx + read_amount]
                        .copy_from_slice(&slice[..read_amount]);
                    event_val_idx += read_amount;
                }

                let increment = u64::from_ne_bytes(event_val_bytes);

                let eventfd = state.global.event_fds.get(&self.eventfd_id).unwrap();
                let current_counter = eventfd.counter;
                let read_polled = eventfd.read_polled.clone();
                let write_polled = eventfd.write_polled.clone();

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
                let new_counter = match current_counter.checked_add(increment) {
                    Some(c) if c != u64::MAX => c,
                    _ => {
                        let poller_id = state.new_poller();
                        state.lower_polled(&write_polled);
                        state.register_poller(poller_id.clone(), write_polled);

                        self.state = EventfdWriteState::Finish(Some(poller_id));
                        return Outcome::Yield(None);
                    }
                };

                state
                    .global
                    .event_fds
                    .get_mut(&self.eventfd_id)
                    .unwrap()
                    .counter = new_counter;

                if new_counter == u64::MAX - 1 {
                    state.lower_polled(&write_polled);
                }
                state.raise_polled(&read_polled);

                Outcome::Success(8)
            }
        }
    }
}
