
- Cross-platform idea--running with Wine?


# Notes

A useful option for reproducing fuzzing behavior in AFL++ is:

`-s seed       - use a fixed seed for the RNG`

This leads to a variable within AFL++ internal state being set:

`OKF("Running with fixed seed: %u", (u32)afl->init_seed);`

Unfortunately, we cannot use this for deriving randomness in Fizzle prior to the first fuzzing round.
Instead, we implement something ourselves.


# FAQ

1. Fizzle keeps crashing with a bus error in my Docker/Podman container or VM!

To share state among processes, Fizzle allocates a sizeable portion of shared memory (around 65MB per harness with standard settings applied).
Containers enforce a maximum of 64MB in /dev/shm. To overcome this, use `--shm-size=<amount>` during `docker run`, where `<amount>` is equal to some memory amount (`1gb` usually works well). This won't immediately consume 1 gigabyte of memory; it will simply give the container the ability to allocate up to that amount as shared memory.

2. I want to run Fizzle with AFL++ in deferred forserver mode, but it isn't working.

Make sure to set the environment variable `AFL_DEFER_FORKSRV=1` when running `afl-fuzz`.
This is necessary as Fizzle performs deferred initialization of the forkserver; as Fizzle is a shared library, `afl-fuzz` can't detect its presence in the binary and so will assume no deferred initialization is available unless you set this flag.

Note that deferred forkserver fuzzing will not work for multi-process fuzzing if deferred forkserver is used.
We recommend using Nyx with Fizzle to fuzz multi-process applications.

3. I want to fuzz a Go binary

Go is unique among languages in that it implements system calls from scratch on Linux instead of linking to libc.
This means that `LD_PRELOAD` will not interpose system calls for Go programs; this is a more fundamental limitation of using LD_PRELOAD.
Thankfully, there's a workaraound to this.

Several operating systems (notably MacOS and Solaris) do not define a stable ABI for syscalls, but instead mandate that applications use the provided standard library.
As a result, Go binaries that are build for these platforms use libc calls instead of raw system calls to communicate with the OS.
To use Fizzle with Go binaries, simply cross-compile the application you would like to fuzz to Solaris or MacOS and run fuzzing within on of these operating systems.

4. I want to fuzz a Go binary, but I don't have source code access to cross-compile and I only have the binary for Linux

Okay, okay, you got me. Fizzle can't fuzz closed-source Go binaries that are built only for Linux, for reasons outlined in (3) and (5). Fizzle is *nearly* universally compatible across programming language/UNIX OS combinations; this is the one exception (unless there are other esoteric languages that re-implement syscalls by themselves; those would fall under this category too).

5. Can I use fizzle against a program that calls raw system calls?

It depends. If the program uses the `syscall()` libc function (as is the case for gRPC), then Fizzle should be able to run your program (so long as it doesn't call a syscall not recognized by Fizzle). If the program uses hand-crafted assembly blocks to call a system call (rare but possible), you're out of luck--Fizzle won't work for your program.