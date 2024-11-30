# Tasks

## Refactor
- state.rs: minor refactor on file plugin loading code (effort: low)
- time.rs: all timers; internals in `scheduler.rs` (effort: very high)
- file.rs: everything (effort: high)
- filesystem.rs: everything (effort: medium)
- io.rs: everything (effort: medium-high)
- pipe.rs: everything (effort: medium-high)

## Immediately Needed Features
- Handling special signals (SIGCHLD for SIGSTOP, SIGIO for async io, SIGPIPE for pipes, getaddrinfo async signal)
- signalfd
- Refactoring of sockets
- C streams (FILE*) implementation

## On the Roadmap
- `fprintf` and similar variadic stream-writing methods

## Nice To Have
- `memfd` backend for ephemeral files to ensure unconstrained storage