use std::mem;

// Environment variables
/// Indicates the shared memory key that child processes should access
pub const FIZZLE_MEMORY_ENV: &str = "FIZZLE_MEMORY";
/// Indicates that the user will be fuzzing a single-process application.
///
/// Setting this may slightly speed up the execution rate of a program due to deferred forkserver initialization.
pub const FIZZLE_SINGLEPROCESS_ENV: &str = "FIZZLE_SINGLEPROCESS";

/// Instructs the fizzle harness to keep running if the main process would exit normally
// pub const FIZZLE_NOEXIT_ENV: &str = "FIZZLE_NOEXIT";

// Static buffers

pub const FIZZLE_AFL_LOOP: usize = 1000;

pub const FIZZLE_MAX_FUZZ_ENDPOINTS: usize = 64;

pub const FIZZLE_MAX_FUZZ_INPUT: usize = 1_048_576; // 1 MB. This can be manually changed if desired, though using plugins that derive entropy from this input is preferred.`

pub const FIZZLE_MAX_EPOLLS: usize = 32;
pub const FIZZLE_MAX_EPOLL_FDS: usize = 128;

pub const FIZZLE_SOMAXCONN: usize = 64;
pub const FIZZLE_MAX_SOCKETS: usize = 512;
pub const FIZZLE_MAX_SOCKADDRS: usize = 128;
pub const FIZZLE_MAX_REUSEPORT: usize = 16;

pub const FIZZLE_MAX_PLUGINS: usize = 128;

pub const FIZZLE_MAX_ANCILLARY: usize = 65536;
pub const FIZZLE_MIN_CONNECTIONLESS: usize =
    65536 + mem::size_of::<libc::sockaddr_storage>() + FIZZLE_MAX_ANCILLARY;

pub const FIZZLE_MAX_EVENTFDS: usize = 128;
pub const FIZZLE_MAX_THREADS: usize = 256;
/// The maximum number of paths to files fizzle emulates.
pub const FIZZLE_MAX_FILE_PATHS: usize = 128;
/// The maximum number of files fizzle can emulate.
pub const FIZZLE_MAX_FILES: usize = 128;
pub const FIZZLE_MAX_OPEN_FILES: usize = 128;
pub const FIZZLE_MAX_DIRS: usize = 64;
pub const FIZZLE_MAX_PIPES: usize = 256;
pub const FIZZLE_MAX_MESSAGE_QUEUES: usize = 256;
pub const FIZZLE_BUFFER_LENGTH: usize = 262_144; // 256 KB per buffer (twice the Linux default for `/proc/sys/net/ipv4/tcp_rmem`)
pub const FIZZLE_MAX_BUFFERS: usize = 256; // 256 * 128 KB = 64 MB total

pub const FIZZLE_MAX_NAMED_SEMAPHORES: usize = 128;
pub const FIZZLE_MAX_FDS: usize = 4096;
pub const FIZZLE_MAX_WAITING_SEMAPHORES: usize = 32;
pub const FIZZLE_FOPEN_BUFSIZE: usize = 4096;

pub const FIZZLE_MAX_PER_ROUND_ENDPOINTS: usize = 128;

// Polling
pub const FIZZLE_MAX_PER_EVENT_QUEUED_POLLERS: usize = 64;
pub const FIZZLE_MAX_PER_POLLER_QUEUED_EVENTS: usize = 64;
pub const FIZZLE_MAX_POLLERS: usize = 128;
pub const FIZZLE_MAX_POLLED_EVENTS: usize = 256;
pub const FIZZLE_MAX_QUEUED_READY_POLLERS: usize = 256;

pub const FIZZLE_EPHEMERAL_PORT_START: u16 = 32768;
pub const FIZZLE_EPHEMERAL_PORT_END: u16 = 61000;

pub const FIZZLE_MAX_PLUGIN_STREAMS: usize = 256;

/// The maximum Process ID that may be used. Note that PIDs 0 and 1 are reserved, so the real
/// maximum number of processes is 2 less than this.
pub const FIZZLE_MAX_PROCESSES: usize = 128;
/// The maximum number of threads in a parent process that can be waiting on a given child.
pub const FIZZLE_MAX_WAITING_PARENTS: usize = 4;
/// The maximum number of processes that have exited and are waiting to be reaped by a parent.
pub const FIZZLE_MAX_DEAD_PROCESSES: usize = 16;
/// The maximum number of process groups (`pgid`) that may be created within a fizzle context.
pub const FIZZLE_MAX_PROCESS_GROUPS: usize = 16;
/// The maximum number of processes that may be part of a process group.
pub const FIZZLE_MAX_PROCESS_GROUP_SIZE: usize = 16;
/// The maximum number of child processes a parent may have.
pub const FIZZLE_MAX_CHILD_PROCESSES: usize = 16;

/// The maximum number of threads/processes that may exist throughout the lifetime of the program
pub const FIZZLE_MAX_WORKERS: usize = 4096;
