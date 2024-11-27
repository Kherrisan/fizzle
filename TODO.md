# TODO

## Immediately Needed Features
- Handling special signals (SIGCHLD for SIGSTOP, SIGIO for async io, SIGPIPE for pipes, getaddrinfo async signal)
- signalfd
- Refactoring of sockets
- C streams (FILE*) implementation

## On the Roadmap
- `fprintf` and similar variadic stream-writing methods

## Nice To Have
- `memfd` backend for ephemeral files to ensure unconstrained storage