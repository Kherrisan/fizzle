// Environment variables

use std::time::Duration;

pub const FIZZLE_ALLOC_OFFSET_ENV: &str = "FIZZLE_ALLOC_OFFSET";
pub const FIZZLE_ALLOC_ENV: &str = "FIZZLE_ALLOC";
/// Indicates the shared memory key that child processes should access
pub const FIZZLE_MEMORY_ENV: &str = "FIZZLE_MEMORY";
pub const FIZZLE_MEMORY_OFFSET_ENV: &str = "FIZZLE_MEMORY_OFFSET";
/// Indicates that the user will be fuzzing a single-process application.
///
/// Setting this may slightly speed up the execution rate of a program due to deferred forkserver initialization.
pub const FIZZLE_SINGLEPROCESS_ENV: &str = "FIZZLE_SINGLEPROCESS";

pub const FIZZLE_TICK_ENV: &str = "FIZZLE_TICK";
pub const FIZZLE_TIMEOUT_ENV: &str = "FIZZLE_TIMEOUT";

pub const FIZZLE_DEFAULT_TICK: Duration = Duration::from_micros(10);
pub const FIZZLE_DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

pub const FIZZLE_HEAP_SIZE: usize = 30 * 1024 * 1024; // 30 MB by default

pub const FIZZLE_STREAM_BUFSIZ: usize = libc::BUFSIZ as usize;

/// Instructs the fizzle harness to keep running if the main process would exit normally
// pub const FIZZLE_NOEXIT_ENV: &str = "FIZZLE_NOEXIT";

// Static buffers

pub const FIZZLE_AFL_LOOP: usize = 1000;

pub const FIZZLE_SOMAXCONN: usize = 64;
pub const FIZZLE_MAX_SOCKADDRS: usize = 128;

/// The maximum number of paths to files fizzle emulates.
pub const FIZZLE_MAX_FILE_PATHS: usize = 128;

pub const FIZZLE_BUFFER_LENGTH: usize = 262_144; // 256 KB per buffer (twice the Linux default for `/proc/sys/net/ipv4/tcp_rmem`)

pub const FIZZLE_MAX_NAMED_SEMAPHORES: usize = 128;

pub const FIZZLE_EPHEMERAL_PORT_START: u16 = 32768;
pub const FIZZLE_EPHEMERAL_PORT_END: u16 = 61000;
