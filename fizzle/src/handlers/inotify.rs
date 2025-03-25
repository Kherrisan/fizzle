use std::cell::RefCell;
use std::ffi::CStr;
use std::os::fd::RawFd;
use std::rc::Rc;

use bitflags::bitflags;

use crate::errno::Errno;
use crate::handlers::descriptor::Descriptor;
use crate::scheduler::{fizzle_alloc, Event, Outcome};
use crate::state::FizzleState;
use crate::GlobalRc;

use super::descriptor::{DescriptorInfo, FdResource};
use super::polled::PolledInfo;

pub struct InotifyInfo {
    pub polled: GlobalRc<PolledInfo>,
}


bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct InotifyEvents: u32 {
        const ACCESS = libc::IN_ACCESS;
        const ATTRIB = libc::IN_ATTRIB;
        const CLOSE_WRITE = libc::IN_CLOSE_WRITE;
        const CLOSE_NOWRITE = libc::IN_CLOSE_NOWRITE;
        const CREATE = libc::IN_CREATE;
        const DELETE = libc::IN_DELETE;
        const DELETE_SELF = libc::IN_DELETE_SELF;
        const MODIFY = libc::IN_MODIFY;
        const MOVE_SELF = libc::IN_MOVE_SELF;
        const MOVED_FROM = libc::IN_MOVED_FROM;
        const MOVED_TO = libc::IN_MOVED_TO;
        const OPEN = libc::IN_OPEN;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct InotifyFlags: libc::c_int {
        const NONBLOCK = libc::IN_NONBLOCK;
        const CLOEXEC = libc::IN_CLOEXEC;
    }
}

pub struct InotifyInitEvent {
    flags: Option<InotifyFlags>,
}

impl<'a> InotifyInitEvent {
    #[inline]
    pub fn new(flags: Option<InotifyFlags>) -> Self {
        Self {
            flags,
        }
    }
}

impl Event for InotifyInitEvent {
    type Success = RawFd;
    type Error = Errno;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        let fd = crate::create_descriptor();
        let descriptor_id = Descriptor::from_raw_fd(fd);

        let nonblocking = self.flags.map(|f| f.contains(InotifyFlags::NONBLOCK)).unwrap_or(false);
        let close_on_exec = self.flags.map(|f| f.contains(InotifyFlags::CLOEXEC)).unwrap_or(false);

        let inotify_info = Rc::new_in(RefCell::new(InotifyInfo {
            polled: Rc::new_in(RefCell::new(PolledInfo {
                pollers: Vec::new_in(fizzle_alloc()),
                event_raised: false,
            }), fizzle_alloc()),
        }), fizzle_alloc());

        state.local.fds.insert(
            descriptor_id,
            DescriptorInfo {
                close_on_exec,
                nonblocking,
                is_passthrough: false,
                resource: FdResource::Inotify(inotify_info),
            },
        );

        Outcome::Success(fd)
    }
}

pub struct InotifyAddWatchEvent<'a> {
    desc: Descriptor,
    pathname: &'a CStr,
    ev_mask: InotifyEvents,
}

impl<'a> InotifyAddWatchEvent<'a> {
    #[inline]
    pub fn new(desc: Descriptor, pathname: &'a CStr, ev_mask: InotifyEvents) -> Self {
        Self {
            desc,
            pathname,
            ev_mask,
        }
    }
}

impl Event for InotifyAddWatchEvent<'_> {
    type Success = RawFd;
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: implement once FS emulation is sorted out
        log::error!("inotify_add_watch() not implemented");
        Outcome::Success(crate::create_inotify_watch())
    }
}

pub struct InotifyRemoveWatchEvent {
    desc: Descriptor,
    wd: libc::c_int,
}

impl InotifyRemoveWatchEvent {
    #[inline]
    pub fn new(desc: Descriptor, wd: libc::c_int) -> Self {
        Self {
            desc,
            wd,
        }
    }
}

impl Event for InotifyRemoveWatchEvent {
    type Success = ();
    type Error = Errno;

    fn run(&mut self, _state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        // TODO: implement once FS emulation is sorted out
        log::error!("inotify_rm_watch() not implemented");
        Outcome::Success(())
    }
}
