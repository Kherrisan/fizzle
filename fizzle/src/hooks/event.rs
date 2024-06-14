use crate::{hook_macros, state::{fd::{FdInfo, FdResource}, identifiers::DescriptorId, EventFdInfo, PolledInfo}};

hook_macros::hook! {
    unsafe fn eventfd(
        initval: libc::c_uint,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_eventfd(ctx) {

        let is_semaphore = (flags & libc::EFD_SEMAPHORE) != 0;
        let close_on_exec = (flags & libc::EFD_CLOEXEC) != 0;
        let nonblocking = (flags & libc::EFD_NONBLOCK) != 0;

        let fd = crate::alias_fd_create();

        let read_polled = ctx.global.polled_events.put(PolledInfo::new());
        let write_polled = ctx.global.polled_events.put(PolledInfo::new_raised());

        let eventfd_id = ctx.global.event_fds.put(EventFdInfo {
            read_polled,
            write_polled,
            is_semaphore,
            counter: initval as u64,
        });

        ctx.local.fds.insert(DescriptorId::new(fd), FdInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::EventFd(eventfd_id),
        });
        

        fd
    }
}
