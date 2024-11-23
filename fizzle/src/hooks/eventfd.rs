use crate::handlers::eventfd::EventfdCreateEvent;
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn eventfd(
        initval: libc::c_uint,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_eventfd(ctx) {

        let is_semaphore = (flags & libc::EFD_SEMAPHORE) != 0;
        let close_on_exec = (flags & libc::EFD_CLOEXEC) != 0;
        let nonblocking = (flags & libc::EFD_NONBLOCK) != 0;

        let mut flags_fmt = String::new();
        if is_semaphore {
            flags_fmt += "EFD_SEMAPHORE";
        }
        if close_on_exec {
            flags_fmt += "|EFD_CLOEXEC";
        }
        if nonblocking {
            flags_fmt += "|EFD_NONBLOCK";
        }

        crate::strace!("eventfd(initval={}, flags={}) -> ...", initval, flags_fmt);
        match Scheduler::handle_event(&mut ctx, EventfdCreateEvent::new(initval, is_semaphore, close_on_exec, nonblocking)) {
            Ok(fd) => {
                crate::strace!("eventfd(initval={}, flags={}) -> {}", initval, flags_fmt, fd);
                fd
            },
            Err(_) => unreachable!(),
        }
    }
}
