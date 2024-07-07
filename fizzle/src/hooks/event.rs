use crate::handlers::descriptor::{DescriptorId, DescriptorInfo, FdResource};
use crate::handlers::eventfd::EventfdInfo;
use crate::handlers::polled::PolledInfo;
use crate::hook_macros;


hook_macros::hook! {
    unsafe fn eventfd(
        initval: libc::c_uint,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_eventfd(ctx) {

        let mut state = ctx.acquire();

        let is_semaphore = (flags & libc::EFD_SEMAPHORE) != 0;
        let close_on_exec = (flags & libc::EFD_CLOEXEC) != 0;
        let nonblocking = (flags & libc::EFD_NONBLOCK) != 0;

        let fd = crate::alias_fd_create();

        let read_polled = state.global.polled_events.allocate(if initval == 0 { PolledInfo::new() } else { PolledInfo::new_raised() }).unwrap();
        let write_polled = state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap();

        let eventfd_id = state.global.event_fds.allocate(EventfdInfo {
            read_polled,
            write_polled,
            is_semaphore,
            counter: initval as u64,
        }).unwrap();

        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd), DescriptorInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::EventFd(eventfd_id),
        }).unwrap();

        fd
    }
}
