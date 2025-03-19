# Fizzle Architecture



## Hooks

The very first layer of Fizzle is the libc functions that it interposes. These represent the
beginning of control flow for Fizzle. Utilities that enable hooks to be compatible with the
operating system's dynamic interposition API (e.g. `LD_PRELOAD` for Linux) can be found in
`src/hook_macros`. The actual hook functions can be found in `src/hooks/*.rs`, while
thread-local variables and functions that are necessary to avoid hook recursion are contained in
`src/hooks.rs`.

The body of individual hook functions has the responsibility of checking the validity of certain
arguments (such as ensuring non-null pointers), coercing arguments int more Rust-friendly types,
and passing those arguments to the `Scheduler` for further action. The value returned by the
scheduler is then coerced into a suitable return value for the function (and `errno` value, if
applicable).

Variadic hook functions must perform two additional actions: set and clear the `entered_handler`
thread-local variable, and extract all needed variadic arguments prior to calling the scheduler

## Scheduler

The Scheduler layer is responsible for ensuring that threads and processes run _sequentially_. In
addition to this, it handles transitions from one thread/process to another and ensures that any
registered cleanup handlers are called on thread/process death. When routines need to be executed
within a specific process's space, the Scheduler ensures that this will happen.


## Handler

The Handler layer handles the business logic associated with each hook function. These routines are
always executed in a single-threaded manner.



### Handling process termination

Unexpected: one of `SIGBUS`, `SIGFPE`, `SIGILL`, `SIGSEGV`, `SIGSYS`, `SIGTRAP`, `SIGXCPU`, or `SIGXFSZ` is raised
- If signal handler receives one of these, send `SIGTERM` to parent and all children

Expected: one of `SIGINT`, `SIGTERM` or `SIGQUIT` is raised
- If signal handler receives one of these, send `SIGTERM` to parent and all children

Other: `SIGCHLD`
- If signal 

root process doesn't send signals to parent
