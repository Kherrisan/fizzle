use crate::state::fd::{FdInfo, FdResource};
use crate::state::{DescriptorId, PipeInfo, PipeMode};
use crate::{hook_macros, RingBuffer};

hook_macros::hook! {
    unsafe fn pipe(
        pipefd: *mut libc::c_int
    ) -> libc::c_int => fizzle_pipe(ctx) {

        let fd1 = crate::alias_fd_create();
        let fd2 = crate::alias_fd_create();

        let buffer1 = ctx.global().buffers.put(RingBuffer::new());
        let buffer2 = ctx.global().buffers.put(RingBuffer::new());

        let pipe1_id = ctx.global().pipes.put(PipeInfo {
            mode: PipeMode::Streamed,
            peer: None,
            read_buf: buffer1,
        });

        let pipe2_id = ctx.global().pipes.put(PipeInfo {
            mode: PipeMode::Streamed,
            peer: Some(pipe1_id),
            read_buf: buffer2,
        });

        // `unwrap()` guaranteed to succeed--we *just* inserted the pipe
        ctx.global().pipes.get_mut(pipe1_id).unwrap().peer = Some(pipe2_id);

        let fd1_info = FdInfo {
            close_on_exec: false,
            nonblocking: false,
            resource: FdResource::Pipe(pipe1_id),
        };

        let fd2_info = FdInfo {
            close_on_exec: false,
            nonblocking: false,
            resource: FdResource::Pipe(pipe2_id),
        };

        // Now add the fd -> pipe_id mapping
        ctx.local().fds.insert(DescriptorId::new(fd1), fd1_info).unwrap();
        ctx.local().fds.insert(DescriptorId::new(fd2), fd2_info).unwrap();

        *pipefd = fd1;
        *(pipefd.add(1)) = fd2;

        0
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

        let buffer1 = ctx.global().buffers.put(RingBuffer::new());
        let buffer2 = ctx.global().buffers.put(RingBuffer::new());

        let pipe1_id = ctx.global().pipes.put(PipeInfo {
            mode,
            peer: None,
            read_buf: buffer1,
        });

        let pipe2_id = ctx.global().pipes.put(PipeInfo {
            mode,
            peer: Some(pipe1_id),
            read_buf: buffer2,
        });

        // `unwrap()` guaranteed to succeed--we *just* inserted the pipe
        ctx.global().pipes.get_mut(pipe1_id).unwrap().peer = Some(pipe2_id);

        let fd1_info = FdInfo {
            close_on_exec,
            nonblocking,
            resource: FdResource::Pipe(pipe1_id),
        };

        let fd2_info = FdInfo {
            close_on_exec,
            nonblocking,
            resource: FdResource::Pipe(pipe2_id),
        };

        // Now add the fd -> pipe_id mapping
        ctx.local().fds.insert(DescriptorId::new(fd1), fd1_info).unwrap();
        ctx.local().fds.insert(DescriptorId::new(fd2), fd2_info).unwrap();

        *pipefd = fd1;
        *(pipefd.add(1)) = fd2;

        0
    }
}
