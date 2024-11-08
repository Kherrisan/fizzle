use crate::handlers::descriptor::{DescriptorId, DescriptorInfo, FdResource};
use crate::handlers::pipe::{PipeInfo, PipeMode};
use crate::handlers::polled::PolledInfo;
use crate::hook_macros;

use fizzle_common::storage::Buffer;

hook_macros::hook! {
    unsafe fn pipe(
        pipefd: *mut libc::c_int
    ) -> libc::c_int => fizzle_pipe(_ctx) {
        fizzle_pipe2(pipefd, 0)
    }
}

hook_macros::hook! {
    unsafe fn pipe2(
        pipefd: *mut libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_pipe2(ctx) {
        let mut state = ctx.acquire();

        let nonblocking = (flags & libc::O_NONBLOCK) != 0;
        let close_on_exec = (flags & libc::O_CLOEXEC) != 0;
        let mode = if (flags & libc::O_DIRECT) != 0 {
            PipeMode::Direct
        } else {
            PipeMode::Streamed
        };

        let fd1 = crate::create_descriptor();
        let fd2 = crate::create_descriptor();

        let first_pipe = PipeInfo {
            mode,
            peer: None,
            read_buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
            read_polled: state.global.polled_events.allocate(PolledInfo::new()).unwrap(),
            write_polled: state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap(),
        };

        let first_pipe_id = state.global.pipes.allocate(first_pipe).unwrap();

        let second_pipe = PipeInfo {
            mode,
            peer: Some(first_pipe_id.clone()),
            read_buf: state.global.buffers.allocate(Buffer::new()).unwrap(),
            read_polled: state.global.polled_events.allocate(PolledInfo::new()).unwrap(),
            write_polled: state.global.polled_events.allocate(PolledInfo::new_raised()).unwrap(),
        };

        let second_pipe_id = state.global.pipes.allocate(second_pipe).unwrap();

        // `unwrap()` guaranteed to succeed--we *just* inserted the pipe
        state.global.pipes.get_mut(&first_pipe_id).unwrap().peer = Some(second_pipe_id.clone());

        let fd1_info = DescriptorInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Pipe(first_pipe_id),
        };

        let fd2_info = DescriptorInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Pipe(second_pipe_id),
        };

        // Now add the fd -> pipe_id mapping
        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd1), fd1_info).unwrap();
        state.local.fds.allocate_with_key(DescriptorId::from_raw_fd(fd2), fd2_info).unwrap();

        *pipefd = fd1;
        *(pipefd.add(1)) = fd2;

        0
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
