use crate::hook_macros;
use crate::state::fd::{FdInfo, FdResource};
use crate::state::identifiers::DescriptorId;
use crate::state::{PipeInfo, PipeMode, PolledInfo};

use fizzle_common::storage::Buffer;

hook_macros::hook! {
    unsafe fn pipe(
        pipefd: *mut libc::c_int
    ) -> libc::c_int => fizzle_pipe(ctx) {
        drop(ctx);
        fizzle_pipe2(pipefd, 0)
    }
}

hook_macros::hook! {
    unsafe fn pipe2(
        pipefd: *mut libc::c_int,
        flags: libc::c_int
    ) -> libc::c_int => fizzle_pipe2(ctx) {

        let nonblocking = (flags & libc::O_NONBLOCK) != 0;
        let close_on_exec = (flags & libc::O_CLOEXEC) != 0;
        let mode = if (flags & libc::O_DIRECT) != 0 {
            PipeMode::Direct
        } else {
            PipeMode::Streamed
        };

        let fd1 = crate::alias_fd_create();
        let fd2 = crate::alias_fd_create();

        let first_pipe = PipeInfo {
            mode,
            peer: None,
            read_buf: ctx.global().buffers.put(Buffer::new()),
            read_polled: ctx.global().polled_events.put(PolledInfo::new()),
            write_polled: ctx.global().polled_events.put(PolledInfo::new_raised()),
        };

        let first_pipe_id = ctx.global().pipes.put(first_pipe);

        let second_pipe = PipeInfo {
            mode,
            peer: Some(first_pipe_id),
            read_buf: ctx.global().buffers.put(Buffer::new()),
            read_polled: ctx.global().polled_events.put(PolledInfo::new()),
            write_polled: ctx.global().polled_events.put(PolledInfo::new_raised()),
        };

        let second_pipe_id = ctx.global().pipes.put(second_pipe);

        // `unwrap()` guaranteed to succeed--we *just* inserted the pipe
        ctx.global().pipes.get_mut(first_pipe_id).unwrap().peer = Some(second_pipe_id);

        let fd1_info = FdInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Pipe(first_pipe_id),
        };

        let fd2_info = FdInfo {
            close_on_exec,
            nonblocking,
            is_passthrough: false,
            resource: FdResource::Pipe(second_pipe_id),
        };

        // Now add the fd -> pipe_id mapping
        ctx.local().fds.insert(DescriptorId::new(fd1), fd1_info).unwrap();
        ctx.local().fds.insert(DescriptorId::new(fd2), fd2_info).unwrap();

        *pipefd = fd1;
        *(pipefd.add(1)) = fd2;

        0
    }
}
