use crate::errno::Errno;
use crate::handlers::pipe::*;
use crate::hook_macros;
use crate::scheduler::Scheduler;

hook_macros::hook! {
    unsafe fn pipe(
        pipefd: *mut libc::c_int
    ) -> libc::c_int => fizzle_pipe(ctx) {
        crate::strace!("pipe(pipefd={:?}) -> ...", pipefd);

        match Scheduler::handle_event(&mut ctx, PipeCreateEvent::new(PipeCreateFlags::empty())) {
            Ok((desc1, desc2)) => {
                *pipefd = desc1.as_raw_fd();
                *(pipefd.add(1)) = desc2.as_raw_fd();

                crate::strace!("pipe(pipefd={:?}) -> 0", pipefd);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn pipe2(
        pipefd: *mut libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_pipe2(ctx) {
        crate::strace!("pipe2(pipefd={:?}, flags={}) -> ...", pipefd, flags);

        let Some(create_flags) = PipeCreateFlags::from_bits(flags) else {
            crate::strace!("pipe2(pipefd={:?}, flags={}) -> -1 (EINVAL)", pipefd, flags);
            Errno::EINVAL.set_errno();
            return -1
        };

        match Scheduler::handle_event(&mut ctx, PipeCreateEvent::new(create_flags)) {
            Ok((desc1, desc2)) => {
                *pipefd = desc1.as_raw_fd();
                *(pipefd.add(1)) = desc2.as_raw_fd();

                crate::strace!("pipe2(pipefd={:?}, flags={:?}) -> 0", pipefd, create_flags);
                0
            },
            Err(()) => unreachable!(),
        }
    }
}

hook_macros::hook! {
    unsafe fn vmsplice(
        _fd: libc::c_int,
        _iov: *const libc::iovec,
        _nr_segs: libc::size_t,
        _flags: libc::c_uint
    ) -> libc::ssize_t => fizzle_vmsplice(_ctx) {
        unimplemented!("vmsplice()")
    }
}
