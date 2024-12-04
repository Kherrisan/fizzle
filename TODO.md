# Tasks

## Refactor
- file.rs: everything (effort: high)
- filesystem.rs: everything (effort: medium)
- check `dup`, `dup2` handling of reference counts

## Eventually Needed Features
- Handling special signal cases (SIGCHLD for SIGSTOP, SIGIO for async io, SIGPIPE for pipes, getaddrinfo async signal)
- signalfd
- Refactoring of sockets
- C streams (FILE*) implementation

## On the Roadmap
- time.rs: all timers; internals in `scheduler.rs` (effort: high)
- `fprintf` and similar variadic stream-writing methods
- posix_mq.rs: everything from scratch
- sysv_mq.rs: everything from scratch

## Nice To Have
- `memfd` backend for ephemeral files to ensure unconstrained storage
- POSIX asynchronous I/O (AIO)