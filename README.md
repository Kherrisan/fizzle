



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
