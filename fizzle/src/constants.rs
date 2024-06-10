// Environment variables
/// Indicates the shared memory key that child processes should access
pub const FIZZLE_MEMORY_ENV: &str = "FIZZLE_MEMORY";
/// Indicates that the user will be fuzzing a single-process application
pub const FIZZLE_MULTIPROCESS_ENV: &str = "FIZZLE_MULTIPROCESS";

/// Instructs the fizzle harness to keep running if the main process would exit normally
pub const FIZZLE_NOEXIT_ENV: &str = "FIZZLE_NOEXIT";

// Static buffers

pub const FIZZLE_MAX_FUZZ_ENDPOINTS: usize = 64;

pub const FIZZLE_MAX_FUZZ_INPUT: usize = 262_144; // 256 KB. This can be manually changed if desired, though using plugins that derive entropy from this input is preferred.`

pub const FIZZLE_MAX_EPOLLS: usize = 32;
pub const FIZZLE_MAX_EPOLL_FDS: usize = 128;

pub const FIZZLE_SOMAXCONN: usize = 64;
pub const FIZZLE_MAX_SOCKETS: usize = 512;
pub const FIZZLE_MAX_SOCKADDRS: usize = 256;

pub const FIZZLE_MAX_PLUGINS: usize = 128;

pub const FIZZLE_MAX_PROCESSES: usize = 128;

pub const FIZZLE_MAX_THREADS: usize = 256;
/// The maximum number of paths to files fizzle emulates.
pub const FIZZLE_MAX_FILE_PATHS: usize = 512;
/// The maximum number of files fizzle can emulate.
pub const FIZZLE_MAX_FILES: usize = 512;
pub const FIZZLE_MAX_DIRS: usize = 256;
pub const FIZZLE_MAX_PIPES: usize = 256;
pub const FIZZLE_MAX_MESSAGE_QUEUES: usize = 256;
pub const FIZZLE_BUFFER_LENGTH: usize = 262_144; // 256 KB per buffer (twice the Linux default for `/proc/sys/net/ipv4/tcp_rmem`)
pub const FIZZLE_MAX_BUFFERS: usize = 256; // 256 * 128 KB = 64 MB total

pub const FIZZLE_MAX_NAMED_SEMAPHORES: usize = 128;
pub const FIZZLE_MAX_FDS: usize = 4096;
pub const FIZZLE_MAX_WAITING_SEMAPHORES: usize = 32;
pub const FIZZLE_FOPEN_BUFSIZE: usize = 8192;

// Polling
pub const FIZZLE_MAX_PER_EVENT_QUEUED_POLLERS: usize = 64;
pub const FIZZLE_MAX_PER_POLLER_QUEUED_EVENTS: usize = 64;
pub const FIZZLE_MAX_POLLERS: usize = 128;
pub const FIZZLE_MAX_POLLED_EVENTS: usize = 128;
pub const FIZZLE_MAX_QUEUED_READY_POLLERS: usize = 256;

pub const FIZZLE_EPHEMERAL_PORT_START: u16 = 32768;
pub const FIZZLE_EPHEMERAL_PORT_END: u16 = 61000;

pub const FIZZLE_MAX_PLUGIN_STREAMS: usize = 256;
