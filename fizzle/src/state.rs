use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque};
use std::io::Write;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::process::Command;
use std::thread::ThreadId;
use std::time::Duration;
use std::{array, env, mem, process, ptr, thread};

use embedded_alloc::TlsfHeap;
use fizzle_common::io::{
    AddressFamily, SocketAddrUnix, SocketType, TransportAddress, TransportProtocol, MAX_PATH_LEN,
};
use fizzle_common::path::{FilePath, SemaphorePath};
use fizzle_common::storage::Buffer;
use fizzle_plugin::{IoEndpointVariant, PluginObject, StreamId};
use fxhash::{FxBuildHasher, FxHashMap};
use heapless::FnvIndexMap;
use rand::rngs::SmallRng;
use rand::SeedableRng;

use crate::arena::{KeyedArena, Rc};
use crate::handlers::time::ItimerInfo;
use crate::{comptime, GlobalList, GlobalMap, GlobalRc, GlobalSet, GlobalVec};
use crate::constants::*;
use crate::errno::Errno;
use crate::handlers::barrier::{BarrierInfo, BarrierPtr};
use crate::handlers::buffer::BufferId;
use crate::handlers::condvar::CondVarPtr;
use crate::handlers::descriptor::{Descriptor, DescriptorInfo, FdResource};
use crate::handlers::directory::DirectoryId;
use crate::handlers::file::*;
use crate::handlers::futex::FutexPtr;
use crate::handlers::fuzz_endpoint::{FuzzEndpointId, FuzzEndpointInfo};
use crate::handlers::id::Worker;
use crate::handlers::mq::{MqId, MqInfo};
use crate::handlers::mutex::{MutexInfo, MutexPtr};
use crate::handlers::pipe::{PipeId, PipeInfo};
use crate::handlers::plugin::{PluginEndpointId, PluginInfo};
use crate::handlers::polled::PolledInfo;
use crate::handlers::poller::PollerInfo;
use crate::handlers::process::*;
use crate::handlers::rwlock::*;
use crate::handlers::semaphore::*;
use crate::handlers::signal::*;
use crate::handlers::socket::{
    LocalAddress, PendingInfo, PendingSocket, ServerSocket, SocketId, SocketInfo, SocketState,
    TransportLocationInfo,
};
use crate::handlers::spinlock::SpinlockPtr;
use crate::handlers::thread::{PThreadRoutine, ThreadInfo, Tid};
use crate::plugins::{IoEmulationType, PluginEndpoint};
use crate::semaphore::Semaphore;

use crate::backend::{FileBackend, FileFeedback, PendingBackend, ServerBackend, StandardFeedback, StdioBackend};

// See `set_entered_handler` and `has_entered_handler`
std::thread_local! {
    static ENTERED_HANDLER: RefCell<bool> = const { RefCell::new(false) };
}

/// Marks the thread as currently executing within a fizzle handler.
pub fn set_entered_handler(entered: bool) {
    ENTERED_HANDLER.with(|e| {
        *e.borrow_mut() = entered;
    });
}

/// Indicates whether the thread is currently executing within a fizzle handler.
///
/// We want to be able to call rust functions that may use syscalls without those leading to
/// infinite recursion. To do so, we keep track of whether we've already hooked the current
/// function using a thread-local variable.
pub fn has_entered_handler() -> bool {
    let mut entered = true;
    ENTERED_HANDLER.with(|e| {
        entered = *e.borrow();
    });
    entered
}

pub fn copy_to_shmem(memfd: RawFd, path: &FilePath<MAX_PATH_LEN>) {
    let in_fd = unsafe {
        libc::open(path.as_cstr().as_ptr(), libc::O_RDONLY)
    };

    if in_fd < 0 {
        panic!("failed to copy file to shared memory--file couldn't be opened: {}", Errno::get_errno())
    }

    let stat_data = unsafe {
        let mut stat_buf: MaybeUninit<libc::stat> = MaybeUninit::uninit();
        if libc::fstat(in_fd, ptr::addr_of_mut!(stat_buf) as *mut libc::stat) != 0 {
            panic!("failed to copy file to shared memory--fstat filure: {}", Errno::get_errno())
        }
        stat_buf.assume_init()
    };

    let length = stat_data.st_size as usize;

    #[cfg(target_os = "linux")]
    unsafe {
        let mut offset = 0;
        while (offset as usize) < length {
            let sent = libc::copy_file_range(memfd, ptr::addr_of_mut!(offset), in_fd, ptr::addr_of_mut!(offset), length - (offset as usize), 0);
            if sent < 0 {
                panic!("failed to copy file to CoW shmem object: {}", Errno::get_errno())
            }
            offset += sent as i64;
        }
    }

    #[cfg(not(target_os = "linux"))]
    unsafe {
        let mut offset = 0;
        let mapped = libc::mmap(ptr::null_mut(), length, libc::PROT_READ | libc::PROT_WRITE, 0, memfd, 0);
        if mapped == libc::MAP_FAILED {
            panic!("failed to mmap() when copying to shared memory: {}", Errno::get_errno())
        }
        while (offset as usize) < length {
            let sent = libc::read(in_fd, mapped, length - offset);
            if sent < 0 {
                panic!("failed to copy file: {}", Errno::get_errno())
            }

            offset += sent as usize;
        }

        libc::munmap(mapped);
    }

    unsafe {
        libc::close(in_fd);
    }
}

pub struct FizzleState {
    pub local: ProcessLocalState,
    pub global: &'static mut InterprocessState,
}

impl FizzleState {
    /// Allocates and initizes all of Fizzle's state.
    pub fn new() -> Self {
        // Initialize the logger to print the current PID/TID with each message
        env_logger::Builder::from_default_env()
            .format(|buf, record| {
                writeln!(
                    buf,
                    "[PID({})|{:?}|{}] {}",
                    process::id(),
                    thread::current().id(),
                    record.level().as_str().to_uppercase(),
                    record.args()
                )
            })
            .init();
        log::info!("Logger initialized");

        // Set signal mask to be inherited by all threads/processes of Fizzle
        let new_set = (SignalSet::SIGPIPE | SignalSet::SIGCHLD).to_sigset();
        let mut old_set = SignalSet::empty().to_sigset();
        assert_eq!(
            // Safety: `new_set` and `old_set` pointers are valid
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_SETMASK,
                    ptr::addr_of!(new_set),
                    ptr::addr_of_mut!(old_set)
                )
            },
            0
        );

        // NOTE: must go before `allocate_global_memory`, as this env variable gets set within it.
        let is_main_process = matches!(env::var(FIZZLE_MEMORY_ENV), Err(_));

        // Allocate shared memory for process-shared state
        let global_uninit = Self::allocate_global_memory();

        // Perform bare-bones initialization of global state (if not done yet)
        let global = if is_main_process {
            InterprocessState::situate(global_uninit)
        } else {
            unsafe { global_uninit.assume_init_mut() }
        };

        // Perform bare-bones initialization of process-local state
        let working_directory =
            FilePath::from_raw_bytes(env::current_dir().unwrap().as_os_str().as_bytes()).unwrap();

        let mut local = ProcessLocalState {
            atexit_handlers: Vec::new(),
            atfork_handlers: Vec::default(),
            awaiting_thread_death: HashMap::default(),
            barriers: HashMap::default(),
            cancelling: None,
            condvars: HashMap::default(),
            dirs: BTreeMap::new(),
            fds: BTreeMap::new_in(global.alloc.alloc()),
            file_objs: HashMap::default(),
            futex_waiters: HashMap::default(),
            itimer_prof: None,
            itimer_real: None,
            itimer_virtual: None,
            main_state: None,
            mutexes: HashMap::default(),
            named_semaphores: HashMap::default(),
            on_exit_handlers: Vec::new(),
            pasture: Default::default(),
            process_info: std::rc::Rc::new_in(RefCell::new(ProcessInfo {
                semaphore: Semaphore::new_rc_in(0, true, global.alloc.alloc()),
                awaiting_death: None,
                pid: Pid::PRIMARY,
                ppid: Pid::INIT,
                pgid: Pgid::from_pid(Pid::PRIMARY),
                signal_handlers: array::from_fn(|_| SigDisposition::Default),
                children: BTreeSet::new_in(global.alloc.alloc()),
            }), global.alloc.alloc()),
            pthreads: HashMap::default(),
            pthread_keys: HashMap::default(),
            pthread_key_values: HashMap::default(),
            pthread_cleanup: HashMap::default(),
            rwlocks: HashMap::default(),
            semaphores: HashMap::default(),
            signals: HashMap::default(),
            spinlocks: HashMap::default(),
            terminated_threads: HashSet::default(),
            thread_locks: Default::default(),
            thread_tids: Default::default(),
            tid_threads: Default::default(),
            // Default umask is 0644
            umask: AccessMode::GROUP_READ | AccessMode::USER_WRITE | AccessMode::USER_EXEC,
            working_directory,
        };

        // TODO: do we need to initialize the INIT Pid?
        /*
        // 1 is the PID for the `init` process
        state.global.ids.allocate_with_key(
            WorkerId::from_id(1),
            WorkerInfo {
                process_id: ProcessId::from(usize::MAX),
                thread_id: thread::current().id(),
            },
        ).unwrap();
        */

        let mut unix_fds: [RawFd; 2] = [0; 2];
        let res = unsafe {
            libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, unix_fds.as_mut_ptr() as *mut i32)
        };
        assert_eq!(res, 0, "failed to create unix socketpair() for passing file descriptors across processes");

        global.unix_write_fd = unix_fds[0];
        global.unix_read_fd = unix_fds[1];

        // Assign the process ID to be used for this process
        if let Some(mut inherited_state) = global.inherited_state.take() {
            let pid = inherited_state.pid;
            let pgid = inherited_state.pgid;

            global.pids.insert(pid, local.process_info.clone());

            let mut process_info = local.process_info.borrow_mut();
            process_info.pid = pid;
            process_info.ppid = inherited_state.ppid;
            process_info.pgid = pgid;
            process_info.signal_handlers = inherited_state.signal_handlers;
            drop(process_info);

            global.process_groups.get_mut(&pgid).unwrap().insert(pid);

            // Receive inherited fds
            mem::swap(&mut local.fds, &mut inherited_state.fds);

            let sigmask = inherited_state.sigmask;
            local.thread_locks.insert(thread::current().id(), Semaphore::new_rc_in(0, true, global.alloc.alloc()));
            local.initialize_thread(Tid::from_raw(pid.as_raw()), Some(sigmask));

        } else {
            let pid = local.process_info.borrow().pid;
            let pgid = local.process_info.borrow().pgid;
            let tid = Tid::from_raw(pid.as_raw());

            global.pids.insert(pid, local.process_info.clone());

            let mut pid_set = BTreeSet::new_in(global.alloc.alloc());
            pid_set.insert(pid);
            global.process_groups.insert(pgid, pid_set);

            local.fds.insert(Descriptor::from_raw_fd(0), DescriptorInfo {
                close_on_exec: false,
                nonblocking: false,
                is_passthrough: false,
                resource: FdResource::Stdin,
            });

            local.fds.insert(Descriptor::from_raw_fd(1), DescriptorInfo {
                close_on_exec: false,
                nonblocking: false,
                is_passthrough: false,
                resource: FdResource::Stdout,
            });

            local.fds.insert(Descriptor::from_raw_fd(2), DescriptorInfo {
                close_on_exec: false,
                nonblocking: false,
                is_passthrough: false,
                resource: FdResource::Stderr,
            });

            local.thread_locks.insert(thread::current().id(), Semaphore::new_rc_in(0, true, global.alloc.alloc()));
            local.initialize_thread(tid, None);
        }

        global.process_locks.insert(local.process_info.borrow().pid, Semaphore::new_rc_in(0, true, global.alloc.alloc()));

        let mut state = Self { local, global };

        // Now that everything else is initialized, time to populate startup processes/plugins.
        if is_main_process {
            let mut onstartup_commands = Vec::new();
            let mut onready_commands = Vec::new();

            // Initialize immediate ("onstartup") commands
            comptime::populate_onstartup_processes(&mut onstartup_commands);

            log::info!("`populate_onready_processes`...");
            // Initialize delayed ("onready") commands
            comptime::populate_onready_processes(&mut onready_commands);

            // Initialize plugins--these need to remain fixed in memory, so we use a Box with in-place initialization.
            let mut endpoints = Vec::new();
            let mut plugins = Vec::new();

            // Initialize plugin endpoints
            log::info!("populating comptime-generated plugins...");
            comptime::populate_plugins(&mut endpoints, &mut plugins);
            log::info!("comptime-generated plugins populated.");

            state.local.main_state = Some(MainProcessState {
                onstartup_commands,
                onready_commands,
                plugins,
                pasture: HashMap::default(),
            });

            log::info!("calling `load_config_mappings()`...");
            state.load_config_mappings(endpoints);
            log::info!("`load_config_mappings()` complete.");
            log::info!("`initialize_main_process()` complete.");
        }

        state
    }

    /// Maps the memory to Fizzle's global shared state, creating a new shared memory object if this
    /// is the primary process.
    fn allocate_global_memory() -> &'static mut MaybeUninit<InterprocessState> {
        let size = mem::size_of::<InterprocessState>();
        let is_singleprocess =
            matches!(env::var(FIZZLE_SINGLEPROCESS_ENV), Ok(s) if s.as_str() == "1");

        if is_singleprocess {
            unsafe {
                let location = libc::mmap(
                    ptr::null_mut(),
                    size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                );

                if location == libc::MAP_FAILED {
                    panic!(
                        "failed to mmap global memory (errno {})",
                        *libc::__errno_location()
                    )
                }

                return &mut *(location as *mut MaybeUninit<InterprocessState>);
            }
        }

        // Shared memory doesn't play well with the forkserver, so we need to make sure that
        // processes are forked *before* any shared memory is created.
        #[cfg(feature = "afl")]
        unsafe {
            crate::__afl_manual_init();
        }

        let memfd = match env::var(FIZZLE_MEMORY_ENV) {
            Ok(var) => {
                log::debug!("attaching to already-created shared memory");
                let memfd: RawFd = var.parse().unwrap();
                memfd
            }
            Err(_) => unsafe {
                log::debug!("allocating public shared memory object...");
                let memfd = InterprocessState::interprocess_shmem_create();
                log::debug!("allocated public shared memory object with fd {}", memfd);
                env::set_var(FIZZLE_MEMORY_ENV, memfd.to_string());

                let ret = libc::ftruncate(memfd, size as i64);
                assert_eq!(ret, 0, "ftruncate() failed for interprocess memory: {}", Errno::get_errno());

                memfd
            }
        };

        let location = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                memfd,
                0
            )
        };

        if location == libc::MAP_FAILED {
            panic!("failed to mmap global memory: {}", Errno::get_errno());
        }

        unsafe {
            &mut *(location as *mut MaybeUninit<InterprocessState>)
        }
    }

    /// Allocates a new Copy-on-Write (CoW) within the main process, returning its identifier.
    pub fn allocate_cow(&mut self) -> CowId {
        let cow_id = self.global.next_cow_id;
        self.global.next_cow_id = CowId::next(&cow_id);
        let cow_fd = InterprocessState::cow_shmem_create(cow_id);

        self.local.main_state.as_mut().unwrap().pasture.insert(cow_id, CowInfo {
            memfd: cow_fd,
        });
        let local_cow_fd = unsafe { libc::fcntl(cow_fd, libc::F_DUPFD_CLOEXEC, 0) };
        assert!(local_cow_fd >= 0, "fcntl(F_DUPFD_CLOEXEC, ...) failed for local_cow_fd: {}", Errno::get_errno());

        self.local.pasture.insert(cow_id, CowInfo {
            memfd: local_cow_fd,
        });

        cow_id
    }

    fn load_config_mappings(&mut self, endpoints: Vec<PluginEndpoint>) {
        let alloc = self.global.alloc.alloc();

        for endpoint in endpoints {
            for _ in 0..endpoint.num_streams {
                let endpoint_variant = endpoint.endpoint_variant.clone();
                match endpoint_variant {
                    IoEndpointVariant::Stdio => {
                        self.global.stdio = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => StdioBackend::Feedback(StandardFeedback {
                                buf: self.global.buffers.allocate(Buffer::new()).unwrap(),
                                read_polled: std::rc::Rc::new_in(RefCell::new(PolledInfo {
                                    pollers: Vec::new_in(alloc),
                                    event_raised: false,
                                }), alloc),
                                write_polled: std::rc::Rc::new_in(RefCell::new(PolledInfo {
                                    pollers: Vec::new_in(alloc),
                                    event_raised: false,
                                }), alloc),
                            }),
                            IoEmulationType::Plugin(module_id) => {
                                StdioBackend::Plugin(self.global.add_plugin(
                                    endpoint.endpoint_variant.clone(),
                                    module_id.clone(),
                                ))
                            }
                            IoEmulationType::Sink => StdioBackend::Sink,
                            IoEmulationType::NullSink => StdioBackend::NullSink,
                            IoEmulationType::Fuzz => {
                                let fuzz_endpoint_id = self.global.add_fuzz_endpoint();
                                StdioBackend::Fuzz(fuzz_endpoint_id)
                            }
                            IoEmulationType::Passthrough => StdioBackend::Passthrough,
                        }
                    }
                    IoEndpointVariant::File(pathbuf) => {
                        let path =
                            FilePath::from_raw_bytes(pathbuf.as_os_str().as_bytes()).unwrap();

                        let inode = self.global.next_inode();
                        let uid = self.global.uid;
                        let gid = self.global.gid;
                        let current_time = self.global.current_time;
                        let cow = self.allocate_cow();

                        let file_info = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => std::rc::Rc::new_in(RefCell::new(
                                FileInfo {
                                    path: path.clone(),
                                    dev_id: 0xfe01,
                                    backend: FileBackend::Feedback(FileFeedback { }),
                                    cow: Some(cow),
                                    inode,
                                    mode: AccessMode::all(),
                                    nlink: 1,
                                    uid,
                                    gid,
                                    atime: current_time,
                                    btime: current_time,
                                    mtime: current_time,
                                    ctime: current_time,
                                }), alloc),
                            IoEmulationType::Plugin(module_id) => {
                                let backend = FileBackend::Plugin(self.global.add_plugin(
                                    endpoint.endpoint_variant.clone(),
                                    module_id.clone(),
                                ));
                                std::rc::Rc::new_in(RefCell::new(FileInfo {
                                        path: path.clone(),
                                        cow: None,
                                        backend,
                                        dev_id: 0xfe01,
                                        inode,
                                        mode: AccessMode::all(),
                                        nlink: 1,
                                        uid,
                                        gid,
                                        atime: current_time,
                                        btime: current_time,
                                        mtime: current_time,
                                        ctime: current_time,
                                    }), alloc)
                            }
                            IoEmulationType::Sink => std::rc::Rc::new_in(RefCell::new(FileInfo {
                                    path: path.clone(),
                                    cow: None,
                                    backend: FileBackend::Sink,
                                    dev_id: 0xfe01,
                                    inode,
                                    mode: AccessMode::all(),
                                    nlink: 1,
                                    uid,
                                    gid,
                                    atime: current_time,
                                    btime: current_time,
                                    mtime: current_time,
                                    ctime: current_time,
                                }), alloc),
                            IoEmulationType::NullSink => std::rc::Rc::new_in(RefCell::new(FileInfo {
                                    path: path.clone(),
                                    cow: None,
                                    backend: FileBackend::NullSink,
                                    dev_id: 0xfe01,
                                    inode,
                                    mode: AccessMode::all(),
                                    nlink: 1,
                                    uid,
                                    gid,
                                    atime: current_time,
                                    btime: current_time,
                                    mtime: current_time,
                                    ctime: current_time,
                                }), alloc),
                            IoEmulationType::Fuzz => {
                                let fuzz_endpoint_id = self.global.add_fuzz_endpoint();
                                let cow = self.allocate_cow();
                                let file_info = std::rc::Rc::new_in(RefCell::new(FileInfo {
                                    path: path.clone(),
                                    cow: Some(cow),
                                    backend: FileBackend::Fuzz(fuzz_endpoint_id),
                                    dev_id: 0xfe01,
                                    inode,
                                    mode: AccessMode::all(),
                                    nlink: 1,
                                    uid,
                                    gid,
                                    atime: current_time,
                                    btime: current_time,
                                    mtime: current_time,
                                    ctime: current_time,
                                }), alloc);

                                file_info
                            }
                            IoEmulationType::Passthrough => std::rc::Rc::new_in(RefCell::new(FileInfo {
                                path: path.clone(),
                                cow: None,
                                backend: FileBackend::Passthrough,
                                dev_id: 0xfe01,
                                inode,
                                mode: AccessMode::all(),
                                nlink: 1,
                                uid,
                                gid,
                                atime: current_time,
                                btime: current_time,
                                mtime: current_time,
                                ctime: current_time,
                            }), alloc),
                        };


                        if self.global.file_paths.insert(path, file_info).is_err() {
                            panic!("failed to insert into file_paths")
                        }
                    }
                    IoEndpointVariant::TcpServer(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => ServerBackend::Plugin(
                                self.global.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(self.global.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.global.add_server(
                            TransportAddress::new_inet(addr, TransportProtocol::Tcp),
                            backend,
                        )
                    }
                    IoEndpointVariant::TcpClient(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => PendingBackend::Plugin(
                                self.global.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(self.global.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        let target_address =
                            TransportAddress::new_inet(addr, TransportProtocol::Tcp);
                        let source_address = self.global
                            .ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.global.per_round_clients
                                .push(PerRoundClientInfo {
                                    source_address,
                                    target_address,
                                    backend: match backend {
                                        PendingBackend::Fuzz(fuzz_endpoint_id) => {
                                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id)
                                        }
                                        PendingBackend::Plugin(plugin_id) => {
                                            PerRoundClientBackend::Plugin(plugin_id)
                                        }
                                        _ => unreachable!(),
                                    },
                                })
                                .unwrap();
                        } else {
                            self.global.add_pending_client(source_address, target_address, backend);
                        }
                    }
                    IoEndpointVariant::UdpServer(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => ServerBackend::Plugin(
                                self.global.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(self.global.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.global.add_server(
                            TransportAddress::new_inet(addr, TransportProtocol::Udp),
                            backend,
                        )
                    }
                    IoEndpointVariant::UdpClient(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => PendingBackend::Plugin(
                                self.global.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(self.global.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        let target_address =
                            TransportAddress::new_inet(addr, TransportProtocol::Udp);
                        let source_address = self.global
                            .ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.global.per_round_clients
                                .push(PerRoundClientInfo {
                                    source_address,
                                    target_address,
                                    backend: match backend {
                                        PendingBackend::Fuzz(fuzz_endpoint_id) => {
                                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id)
                                        }
                                        PendingBackend::Plugin(plugin_id) => {
                                            PerRoundClientBackend::Plugin(plugin_id)
                                        }
                                        _ => unreachable!(),
                                    },
                                })
                                .unwrap();
                        } else {
                            self.global.add_pending_client(source_address, target_address, backend);
                        }
                    }
                    IoEndpointVariant::SctpServer(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => ServerBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => ServerBackend::Plugin(
                                self.global.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => ServerBackend::Sink,
                            IoEmulationType::NullSink => ServerBackend::NullSink,
                            IoEmulationType::Fuzz => ServerBackend::Fuzz(self.global.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => ServerBackend::Passthrough,
                        };

                        self.global.add_server(
                            TransportAddress::new_inet(addr, TransportProtocol::Sctp),
                            backend,
                        )
                    }
                    IoEndpointVariant::SctpClient(addr) => {
                        let backend = match &endpoint.emulation_type {
                            IoEmulationType::Feedback => PendingBackend::Feedback(()),
                            IoEmulationType::Plugin(module_id) => PendingBackend::Plugin(
                                self.global.add_plugin(endpoint_variant.clone(), module_id.clone()),
                            ),
                            IoEmulationType::Sink => PendingBackend::Sink,
                            IoEmulationType::NullSink => PendingBackend::NullSink,
                            IoEmulationType::Fuzz => PendingBackend::Fuzz(self.global.add_fuzz_endpoint()),
                            IoEmulationType::Passthrough => PendingBackend::Passthrough,
                        };

                        let target_address =
                            TransportAddress::new_inet(addr, TransportProtocol::Sctp);
                        let source_address = self.global
                            .ephemeral_address(target_address.family(), target_address.protocol());
                        if endpoint.is_per_round {
                            self.global.per_round_clients
                                .push(PerRoundClientInfo {
                                    source_address,
                                    target_address,
                                    backend: match backend {
                                        PendingBackend::Fuzz(fuzz_endpoint_id) => {
                                            PerRoundClientBackend::Fuzz(fuzz_endpoint_id)
                                        }
                                        PendingBackend::Plugin(plugin_id) => {
                                            PerRoundClientBackend::Plugin(plugin_id)
                                        }
                                        _ => unreachable!(),
                                    },
                                })
                                .unwrap();
                        } else {
                            self.global.add_pending_client(source_address, target_address, backend);
                        }
                    }
                    _ => panic!("unimplemented IoEndpoint type"),
                }
            }
        }
    }

    /// Indicates whether the given polled event is ready to be acted on.
    pub fn polled_is_ready(&self, polled: &GlobalRc<PolledInfo>) -> bool {
        polled.borrow().event_raised
    }

    /// Marks the given polled event as ready.
    ///
    /// If not already raised, this method will push_back a poller waiting on this polled event
    /// (if such a poller exists).
    pub fn raise_polled(&mut self, polled: &GlobalRc<PolledInfo>) {
        self.global.raise_polled(polled);
    }

    // if buffer is empty, then call this
    pub fn lower_polled(&mut self, polled: &GlobalRc<PolledInfo>) {
        let mut borrow = polled.borrow_mut();
        debug_assert!(borrow.event_raised);
        borrow.event_raised = false;
    }

    /// Creates a new poller for the currently executing worker.
    pub fn new_poller(&mut self) -> GlobalRc<PollerInfo> {
        let alloc = self.global.alloc.alloc();
        let worker_id = self.current_worker();

        std::rc::Rc::new_in(RefCell::new(PollerInfo {
                worker: worker_id,
                polled_events: Vec::new_in(alloc),
                raised_events: BTreeSet::new_in(alloc),
        }), alloc)
    }

    /// Registers `poller_id` as waiting on `polled_id`.
    pub fn register_poller(&mut self, poller: GlobalRc<PollerInfo>, polled: GlobalRc<PolledInfo>) {
        poller.borrow_mut().polled_events.push(polled.clone());
        let mut polled_borrow = polled.borrow_mut();
        debug_assert!(!polled_borrow.event_raised);
        polled_borrow.pollers.push(poller.clone());
    }

    // Ugh. This looks like O(n^2)...
    /// Deletes the given poller, removing any references to it from `Polled` objects.
    pub fn delete_poller(&mut self, poller: GlobalRc<PollerInfo>) {

        if poller.borrow().in_raised_queue() {
            // Remove the poller from the ready queue, leaving the others in the same order
            self.global.ready.retain(|r| match &r.info {
                ReadyInfo::Poller(p) => poller.borrow().worker != p.borrow().worker,
                _ => true,
            });
        }

        // Remove the poller from each polled instance it was registered to
        for polled in poller.borrow().polled_events.iter() {
            for i in 0..polled.borrow().pollers.len() {
                if polled.borrow().pollers.get(i).unwrap().borrow().worker == poller.borrow().worker {
                    polled.borrow_mut().pollers.remove(i);
                }
            }
        }
    }

    pub fn mark_thread_ready(&mut self, thread_id: ThreadId) {
        let pid = self.local.process_info.borrow().pid;

        let timestamp = self.global.current_time;
        let ready = ReadyInfo::Worker(Worker {
            pid,
            thread_id
        });

        self.global.ready.retain(|r| &r.info != &ready);
        self.global.ready.push(ReadyItem {
            timestamp,
            info: ready,
        });
    }

    pub fn mark_worker_ready(&mut self, worker: Worker) {
        let timestamp = self.global.current_time;
        let ready = ReadyInfo::Worker(worker);

        self.global.ready.retain(|r| &r.info != &ready);
        self.global.ready.push(ReadyItem {
            timestamp,
            info: ready,
        });
    }

    pub fn current_worker(&mut self) -> Worker {
        Worker {
            pid: self.local.process_info.borrow().pid,
            thread_id: thread::current().id(),
        }
    }
}

pub struct InheritedState {
    pub fds: GlobalMap<Descriptor, DescriptorInfo>,
    pub pid: Pid,
    pub ppid: Pid,
    pub pgid: Pgid,
    pub signal_handlers: SignalHandlers,
    pub sigmask: SignalSet,
}

/// State specific to the first (root) process instantiated by Fizzle.
pub struct MainProcessState {
    pub onstartup_commands: Vec<Command>,
    pub onready_commands: Vec<Command>,
    pub plugins: Vec<std::rc::Rc<RefCell<dyn PluginObject>>>,
    pub pasture: HashMap<CowId, CowInfo>,
}

impl Debug for MainProcessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainProcessState")
            .field("onstartup_commands", &self.onstartup_commands)
            .field("awaiting_thread_death", &self.onready_commands)
            .field("plugins", &"<opaque>")
            .finish()
    }
}

pub struct ProcessLocalState {
    /// See `atexit()`
    pub atexit_handlers: Vec<AtExitFunction>,
    /// See `atfork()`
    pub atfork_handlers: Vec<AtForkInfo>,
    /// Indicates which thread(s) are awaiting the death of a specific thread (via pthread_join)
    pub awaiting_thread_death: HashMap<ThreadId, Vec<ThreadId>, FxBuildHasher>,
    pub barriers: HashMap<BarrierPtr, BarrierInfo, FxBuildHasher>,
    /// A thread that has received a cancellation request.
    pub cancelling: Option<ThreadId>,
    pub condvars: HashMap<CondVarPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub dirs: BTreeMap<DirectoryId, FilePath<MAX_PATH_LEN>>,
    pub fds: GlobalMap<Descriptor, DescriptorInfo>,
    /// Files specifically designated as being emulated.
    pub file_objs: HashMap<FilePtr, FileObject, FxBuildHasher>,
    pub futex_waiters: HashMap<FutexPtr, VecDeque<(u32, ThreadId)>, FxBuildHasher>,
    /// The interval between `ITIMER_REAL` events.
    pub itimer_real: Option<ItimerInfo>,
    /// The interval between `ITIMER_VIRTUAL` events.
    pub itimer_virtual: Option<ItimerInfo>,
    /// The interval between `ITIMER_PROF` events.
    pub itimer_prof: Option<ItimerInfo>,
    /// State associated with the main process (e.g. the first process instantiated with the Fizzle harness).
    pub main_state: Option<MainProcessState>,
    pub mutexes: HashMap<MutexPtr, MutexInfo, FxBuildHasher>,
    pub named_semaphores: HashMap<SemaphorePtr, GlobalRc<SemaphoreInfo>>,
    /// See `on_exit()`
    pub on_exit_handlers: Vec<(OnExitFunction, *mut libc::c_void)>,
    pub pasture: HashMap<CowId, CowInfo>,
    pub process_info: GlobalRc<ProcessInfo>,
    pub pthreads: HashMap<libc::pthread_t, ThreadInfo, FxBuildHasher>,
    pub pthread_cleanup: HashMap<ThreadId, VecDeque<PThreadRoutine>, FxBuildHasher>,
    pub pthread_keys: HashMap<libc::pthread_key_t, PThreadRoutine, FxBuildHasher>,
    pub pthread_key_values: HashMap<
        libc::pthread_key_t,
        HashMap<ThreadId, *mut libc::c_void, FxBuildHasher>,
        FxBuildHasher,
    >,
    pub rwlocks: HashMap<RwLockPtr, RwLockInfo, FxBuildHasher>,
    pub semaphores: HashMap<SemaphorePtr, SemaphoreInfo>,
    pub signals: HashMap<ThreadId, ThreadSigInfo, FxBuildHasher>,
    pub spinlocks: HashMap<SpinlockPtr, VecDeque<ThreadId>, FxBuildHasher>,
    pub terminated_threads: HashSet<ThreadId, FxBuildHasher>,
    /// Per-thread semaphores for synchronization.
    pub thread_locks: FxHashMap<ThreadId, std::rc::Rc<Semaphore, &'static TlsfHeap>>,
    pub thread_tids: HashMap<ThreadId, Tid>,
    pub tid_threads: HashMap<Tid, ThreadId>,
    /// The current default permissions mask of the process.
    pub umask: AccessMode,
    /// The directory that the program is currently executing relative to.
    pub working_directory: FilePath<MAX_PATH_LEN>,
}

impl ProcessLocalState {
    pub fn initialize_thread(&mut self, tid: Tid, sigmask: Option<SignalSet>) {

        // Insert the current (main) pthread into `pthreads`
        self.pthreads.insert(
            unsafe { libc::pthread_self() },
            ThreadInfo::new(thread::current().id(), false, true),
        );

        self.signals.insert(
            thread::current().id(),
            ThreadSigInfo {
                raised: array::from_fn(|_| None),
                blocked: sigmask.unwrap_or(SignalSet::empty()),
                interrupted: false,
                sigwait_set: SignalSet::empty(),
                sigsuspend: false,
            },
        );

        self.thread_tids.insert(thread::current().id(), tid);
        self.tid_threads.insert(tid, thread::current().id());
    }
}

pub struct InterprocessState {
    pub alloc: InterprocessAllocator,
    pub process_locks: GlobalMap<Pid, std::rc::Rc<Semaphore, &'static TlsfHeap>>,
    /// The thread identifier to be executed by the waking process. This is `Some` if and only if
    /// a thread is currently about to be scheduled.
    pub waking_id: Option<ThreadId>,
    /// The thread/process identifier to be reaped. This is `Some` if and only if a thread/process
    /// is currently exiting.
    pub exiting_id: Option<Worker>,
    /// State passed between calls to the `exec()` family of functions
    pub inherited_state: Option<InheritedState>,
    /// The thread/process identifier to be signalled with the given signal value. This is `Some`
    /// if and only if a thread is about to receive an outstanding signal.
    pub signal: Option<(SignalDestination, RaisedSignalInfo)>,
    /// Indicates a Copy-on-Write file should be passed to the given worker.
    pub create_cow: Option<CreateCowSource>,
    // TODO: use an env variable to pass this from parent to child when receiving shared memory
    /// The read end of the Unix socket pair used to pass file descriptors between processes.
    pub unix_read_fd: RawFd,
    /// The write end of the Unix socket pair used to pass file descriptors between processes.
    pub unix_write_fd: RawFd,
    
    pub next_pid: Pid,
    pub pids: GlobalMap<Pid, GlobalRc<ProcessInfo>>,
    /// Information on a process that has died but not yet been reaped.
    pub dead_pids: GlobalMap<Pid, SigChildInfo>,

    pub process_groups: GlobalMap<Pgid, GlobalSet<Pid>>,
    pub next_inode: libc::ino_t,
    /// The number of rounds to run fuzzing when executing in Persistent mode.
    pub persistent_rounds: usize,
    /// The next StreamId available to be assigned to an emulated stream.
    pub next_stream_id: StreamId,

    pub next_cow_id: CowId,
    /// The next ephemeral port to be assigned to a socket.
    pub next_ephemeral_port: u16,
    /// If true, stderr is silently dropped; otherwise it is printed.
    pub mask_stderr: bool,
    pub afl_shmem_initialized: bool,
    // TODO: BTreeMap would be unwise--FilePath has an expensive `eq` comparison
    pub file_paths: FnvIndexMap<FilePath<MAX_PATH_LEN>, GlobalRc<FileInfo>, FIZZLE_MAX_FILE_PATHS>,
    pub open_files: KeyedArena<OpenFileId, OpenFileInfo, FIZZLE_MAX_OPEN_FILES>,
    // TODO: BTreeMap would be unwise--SemaphorePath has an expensive `eq` comparison
    pub sem_paths: FnvIndexMap<SemaphorePath, GlobalRc<SemaphoreInfo>, FIZZLE_MAX_NAMED_SEMAPHORES>,
    pub pipes: KeyedArena<PipeId, PipeInfo, FIZZLE_MAX_PIPES>,
    pub message_queues: KeyedArena<MqId, MqInfo, FIZZLE_MAX_MESSAGE_QUEUES>,
    // TODO: SO_REUSEPORT breaks this...
    pub socket_locations:
        FnvIndexMap<TransportAddress, TransportLocationInfo, FIZZLE_MAX_SOCKADDRS>,
    pub sockets: KeyedArena<SocketId, SocketInfo, FIZZLE_MAX_SOCKETS>,
    pub buffers: KeyedArena<BufferId, Buffer<FIZZLE_BUFFER_LENGTH>, FIZZLE_MAX_BUFFERS>,
    pub stdio: StdioBackend,
    /// Polling infrastructure
    pub plugins: KeyedArena<PluginEndpointId, PluginInfo, FIZZLE_MAX_PLUGIN_STREAMS>,
    /// Pollers/Workers that can be immediately scheduled.
    pub ready: BinaryHeap<ReadyItem, &'static TlsfHeap>,
    /// Pollers/Workers that should be scheduled once the system has reached a halted state.
    pub ready_delayed: GlobalList<ReadyInfo>,
    pub fuzz_input: GlobalVec<u8>,
    pub per_round_clients: heapless::Vec<PerRoundClientInfo, FIZZLE_MAX_PER_ROUND_ENDPOINTS>,
    pub per_round_endpoints: GlobalVec<GlobalRc<SocketInfo>>,
    pub fuzz_endpoints: KeyedArena<FuzzEndpointId, FuzzEndpointInfo, FIZZLE_MAX_FUZZ_ENDPOINTS>,
    pub prefuzz_rng: rand::rngs::SmallRng,
    pub current_time: Duration,
    pub time_fuzz_idx: usize,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
}

impl InterprocessState {
    pub fn interprocess_shmem_create() -> RawFd {
        let filename = format!("/Fizzle_Interprocess{}\0", process::id());

        let fd = unsafe {
            libc::shm_open(filename.as_ptr() as *const i8, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, libc::S_IRUSR | libc::S_IWUSR)
        };

        assert!(fd >= 0, "shm_open() failed: {}", Errno::get_errno());

        unsafe {
            assert_eq!(libc::shm_unlink(filename.as_ptr() as *const i8), 0, "shm_unlink() failed: {}", Errno::get_errno());
        }

        let non_cloexec_fd = unsafe { libc::dup(fd) };
        assert!(non_cloexec_fd >= 0, "dup() failed during interprocess file creation: {}", Errno::get_errno());

        unsafe {
            libc::close(fd);
        }

        non_cloexec_fd
    }

    pub fn cow_shmem_create(id: CowId) -> RawFd {
        let filename = format!("/Fizzle_Process{}_CoW{}\0", process::id(), usize::from(id));

        let fd = unsafe {
            libc::shm_open(filename.as_ptr() as *const i8, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, libc::S_IRUSR | libc::S_IWUSR)
        };

        assert!(fd >= 0, "shm_open() failed: {}", Errno::get_errno());

        unsafe {
            assert_eq!(libc::shm_unlink(filename.as_ptr() as *const i8), 0, "shm_unlink() failed: {}", Errno::get_errno());
        }

        fd
    }

    // TODO: situate() is unsafe--whenever we change the fields in InterprocessState, it becomes
    // unsound until we add the corresponding definition. We should really change it to a trait +
    // proc macro derive.
    /// Takes an uninitialized InterprocessState and initializes it in place.
    fn situate(state: &mut MaybeUninit<InterprocessState>) -> &mut InterprocessState {
        unsafe {
            let state = state.as_mut_ptr();
            *ptr::addr_of_mut!((*state).waking_id) = None;
            *ptr::addr_of_mut!((*state).exiting_id) = None;
            *ptr::addr_of_mut!((*state).inherited_state) = None;

            *ptr::addr_of_mut!((*state).mask_stderr) = false;
            *ptr::addr_of_mut!((*state).signal) = None;

            *ptr::addr_of_mut!((*state).persistent_rounds) = FIZZLE_AFL_LOOP; // TODO: make configurable
            *ptr::addr_of_mut!((*state).next_stream_id) = StreamId::from(0);
            *ptr::addr_of_mut!((*state).next_ephemeral_port) = FIZZLE_EPHEMERAL_PORT_START;
            *ptr::addr_of_mut!((*state).next_cow_id) = CowId::first();

            *ptr::addr_of_mut!((*state).next_inode) = 1_000_000;

            *ptr::addr_of_mut!((*state).afl_shmem_initialized) = false;
            *ptr::addr_of_mut!((*state).file_paths) = FnvIndexMap::new();
            *ptr::addr_of_mut!((*state).sem_paths) = FnvIndexMap::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).pipes));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).message_queues));
            *ptr::addr_of_mut!((*state).socket_locations) = FnvIndexMap::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).sockets));
            KeyedArena::initialize(ptr::addr_of_mut!((*state).buffers));

            KeyedArena::initialize(ptr::addr_of_mut!((*state).open_files));

            *ptr::addr_of_mut!((*state).stdio) = StdioBackend::Passthrough;
            KeyedArena::initialize(ptr::addr_of_mut!((*state).plugins));
            *ptr::addr_of_mut!((*state).per_round_clients) = heapless::Vec::new();
            KeyedArena::initialize(ptr::addr_of_mut!((*state).fuzz_endpoints));
            *ptr::addr_of_mut!((*state).prefuzz_rng) =
                SmallRng::seed_from_u64(0xABAD_5EED_ABAD_5EED_u64); // TODO: enable custom seed loading
            *ptr::addr_of_mut!((*state).current_time) = Duration::from_secs(1735924847); // TODO: set this randomly each fuzzing round
            *ptr::addr_of_mut!((*state).uid) = 1000; // TODO: make this configurable
            *ptr::addr_of_mut!((*state).gid) = 1000; // TODO: make this configurable

            // Initialize interprocess allocator
            *ptr::addr_of_mut!((*state).alloc.heap) = TlsfHeap::empty();
            (*ptr::addr_of_mut!((*state).alloc.heap)).init((ptr::addr_of_mut!((*state).alloc.heap_memory)) as usize, FIZZLE_HEAP_SIZE);

            // SAFETY: must happen *after* interprocess allocator has been initialized
            *ptr::addr_of_mut!((*state).per_round_endpoints) = Vec::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).dead_pids) = BTreeMap::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).pids) = BTreeMap::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).process_groups) = BTreeMap::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).process_locks) = BTreeMap::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).ready) = BinaryHeap::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).ready_delayed) = LinkedList::new_in((*state).alloc.alloc());
            *ptr::addr_of_mut!((*state).fuzz_input) = Vec::new_in((*state).alloc.alloc());

            *ptr::addr_of_mut!((*state).unix_read_fd) = -1;
            *ptr::addr_of_mut!((*state).unix_write_fd) = -1;
            *ptr::addr_of_mut!((*state).create_cow) = None;
            *ptr::addr_of_mut!((*state).time_fuzz_idx) = 0;

            *ptr::addr_of_mut!((*state).next_pid) = Pid::PRIMARY.next();
            &mut (*state)
        }
    }

    /// Returns the next available PID.
    pub fn next_pid(&mut self) -> Pid {
        let pid = self.next_pid;
        self.next_pid = pid.next();
        pid
    }

    /// Returns the next available TID.
    pub fn next_tid(&mut self) -> Tid {
        let pid = self.next_pid;
        self.next_pid = pid.next();
        Tid::from_raw(pid.as_raw())
    }

    /// Marks the given polled event as ready.
    ///
    /// If not already raised, this method will push_back a poller waiting on this polled event
    /// (if such a poller exists).
    pub fn raise_polled(&mut self, polled: &GlobalRc<PolledInfo>) {
        let mut polled_borrow = polled.borrow_mut();
        if !polled_borrow.event_raised {
            polled_borrow.event_raised = true;
            let pollers = &mut polled_borrow.pollers;
            for poller in pollers.iter() {
                if !poller.borrow().in_raised_queue() {
                    let timestamp = self.current_time;
                    self.ready.push(ReadyItem {
                        info: ReadyInfo::Poller(poller.clone()),
                        timestamp,
                    });
                }

                poller.borrow_mut().raised_events.insert(polled.clone());
            }
        }
    }

    pub fn next_inode(&mut self) -> libc::ino_t {
        let inode = self.next_inode;
        self.next_inode += 1;
        inode
    }

    pub fn add_fuzz_endpoint(&mut self) -> Rc<FuzzEndpointId> {
        let alloc = self.alloc.alloc();

        let read_polled = std::rc::Rc::new_in(RefCell::new(PolledInfo {
            pollers: Vec::new_in(alloc),
            event_raised: false,
        }), alloc);

        self.fuzz_endpoints
            .allocate(FuzzEndpointInfo {
                read_polled,
                read_idx: 0,
            })
            .unwrap()
    }

    pub fn add_pending_client(
        &mut self,
        src_addr: TransportAddress,
        rem_addr: TransportAddress,
        backend: PendingBackend,
    ) -> GlobalRc<SocketInfo> {
        let alloc = self.alloc.alloc();

        let client_socket_info = std::rc::Rc::new_in(RefCell::new(SocketInfo {
            fd_count: 0,
            state: SocketState::PendingConnection(PendingSocket {
                rem_addr: rem_addr.clone(),
                backend,
                next_pending: None,
            }),
            socktype: SocketType::Datagram,
            protocol: src_addr.protocol(),
            local_addr: LocalAddress::Assigned(src_addr.addr().clone()),
        }), self.alloc.alloc());

        // Add the client to the pending client chain, if applicable
        match self.socket_locations.get_mut(&rem_addr) {
            None => {
                let polled = std::rc::Rc::new_in(RefCell::new(PolledInfo {
                    pollers: Vec::new_in(alloc),
                    event_raised: false,
                }), alloc);
                if self.socket_locations
                    .insert(
                        rem_addr,
                        TransportLocationInfo {
                            reuse_port: false,
                            bound_sockets: LinkedList::new_in(self.alloc.alloc()),
                            pending: Some(PendingInfo {
                                client: client_socket_info.clone(),
                                poll: polled,
                            }),
                        },
                    ).is_err() {
                    panic!("failed to insert to socket_locations")
                }
            }
            Some(location_info) => {
                match &location_info.pending {
                    Some(PendingInfo { client, .. }) => {
                        let mut last_client = client.clone();
                        let mut last_client_borrow = last_client.borrow();
                        while let SocketState::PendingConnection(PendingSocket {
                            next_pending: Some(id),
                            ..
                        }) = &last_client_borrow.state {
                            let new_last_client = id.clone();
                            drop(last_client_borrow);
                            last_client = new_last_client;
                            last_client_borrow = last_client.borrow();
                        }

                        let SocketState::PendingConnection(PendingSocket {
                            next_pending: next_awaiting,
                            ..
                        }) = &mut last_client.borrow_mut().state
                        else {
                            panic!("unexpected internal fizzle state--chain of awaiting clients had invalid socket variant")
                        };

                        *next_awaiting = Some(client_socket_info.clone());
                    }
                    None => {
                        let polled = std::rc::Rc::new_in(RefCell::new(PolledInfo {
                            pollers: Vec::new_in(alloc),
                            event_raised: false,
                        }), alloc);
                        location_info.pending = Some(PendingInfo {
                            client: client_socket_info.clone(),
                            poll: polled,
                        });
                    }
                }

                if let Some(socket_info) = location_info.bound_sockets.pop_front() {
                    log::debug!("found bound socket at location for pending connection");

                    location_info.bound_sockets.push_back(socket_info.clone());

                    match &socket_info.borrow().state {
                        SocketState::Server(server_info) => {
                            log::debug!("notifying server that pending connection exists...");
                            let connect_poll = server_info.ready_to_connect.clone();
                            self.raise_polled(&connect_poll);
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }

        client_socket_info
    }

 
    pub fn add_server(&mut self, transport_addr: TransportAddress, backend: ServerBackend) {
        let alloc = self.alloc.alloc();

        // Create a new polled instance for listeners waiting to accept connections
        let connect_polled = std::rc::Rc::new_in(RefCell::new(PolledInfo {
            pollers: Vec::new_in(alloc),
            event_raised: false,
        }), alloc);

        let socket_info = std::rc::Rc::new_in(RefCell::new(SocketInfo {
            fd_count: 0,
            state: SocketState::Server(ServerSocket {
                backend,
                connecting: LinkedList::new_in(self.alloc.alloc()),
                ready_to_connect: connect_polled,
            }),
            socktype: SocketType::Datagram, // TODO: this (and above) aren't necessarily true
            protocol: transport_addr.protocol(),
            local_addr: LocalAddress::Assigned(transport_addr.addr().clone()),
        }), self.alloc.alloc());
        
        match self.socket_locations.get_mut(&transport_addr) {
            None => {
                let mut bound_sockets = LinkedList::new_in(self.alloc.alloc());
                bound_sockets.push_back(socket_info);

                if self.socket_locations
                    .insert(
                        transport_addr.clone(),
                        TransportLocationInfo {
                            pending: None,
                            reuse_port: false,
                            bound_sockets,
                        },
                    ).is_err() {
                        panic!("failed to insert to socket_locations")
                    }
            }
            Some(location_info) => {
                debug_assert!(location_info.bound_sockets.is_empty());
                location_info.bound_sockets.push_back(socket_info);
            }
        };
    }

    pub fn add_plugin(
        &mut self,
        endpoint: IoEndpointVariant,
        module: std::rc::Rc<RefCell<dyn PluginObject>>,
    ) -> Rc<PluginEndpointId> {
        let alloc = self.alloc.alloc();

        let stream = self.next_stream_id;
        self.next_stream_id = StreamId::from(usize::from(stream) + 1);

        let read_buf = self.buffers.allocate(Buffer::new()).unwrap();
        let read_polled = std::rc::Rc::new_in(RefCell::new(PolledInfo {
            pollers: Vec::new_in(alloc),
            event_raised: false,
        }), alloc);

        let write_buf = self.buffers.allocate(Buffer::new()).unwrap();
        let write_polled = std::rc::Rc::new_in(RefCell::new(PolledInfo {
            pollers: Vec::new_in(alloc),
            event_raised: true,
        }), alloc);

        self.plugins
            .allocate(PluginInfo {
                endpoint,
                stream,
                module,
                read_buf,
                read_polled,
                write_buf,
                write_polled,
            })
            .unwrap()
    }

    /// Assigns the next available ephemeral address.
    pub fn ephemeral_address(
        &mut self,
        family: AddressFamily,
        protocol: TransportProtocol,
    ) -> TransportAddress {
        match family {
            AddressFamily::Ipv4 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    // TODO: `panic`s like these won't actually crash the system if they're in subprocesses...
                    // Use a panic handler to kill primary process?
                    panic!("all ephemeral ports were exhausted");
                    // self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                // TODO: use an address other than 127.0.0.1 or ::1?
                TransportAddress::new_inet(
                    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)),
                    protocol,
                )
            }
            AddressFamily::Ipv6 => {
                let port = self.next_ephemeral_port;
                if self.next_ephemeral_port >= FIZZLE_EPHEMERAL_PORT_END {
                    self.next_ephemeral_port = FIZZLE_EPHEMERAL_PORT_START;
                } else {
                    self.next_ephemeral_port += 1;
                }
                TransportAddress::new_inet(
                    SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
                        port,
                        0,
                        0,
                    )),
                    protocol,
                )
            }
            AddressFamily::Unix => TransportAddress::new_unix(SocketAddrUnix::Unnamed),
        }
    }
}

pub struct InterprocessAllocator {
    heap: TlsfHeap,
    heap_memory: [MaybeUninit<u8>; FIZZLE_HEAP_SIZE],
}

impl InterprocessAllocator {
    pub fn alloc<'a>(&'a self) -> &'static TlsfHeap {
        // Safety: `self.heap` is never mutably referenced outside of initialization, and lives
        // until the end of the program. This means that static shared references to it are safe.
        unsafe {
            mem::transmute::<&'a TlsfHeap, &'static TlsfHeap>(&self.heap)
        }
    }
}

#[derive(Debug)]
pub struct PerRoundClientInfo {
    pub source_address: TransportAddress,
    pub target_address: TransportAddress,
    pub backend: PerRoundClientBackend,
}

#[derive(Clone, Debug)]
pub enum PerRoundClientBackend {
    Fuzz(Rc<FuzzEndpointId>),
    Plugin(Rc<PluginEndpointId>),
}

#[derive(Clone)]
pub struct ReadyItem {
    pub info: ReadyInfo,
    pub timestamp: Duration,
}

impl PartialEq for ReadyItem {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for ReadyItem {}

impl PartialOrd for ReadyItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReadyItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp.cmp(&other.timestamp)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum ReadyInfo {
    Poller(GlobalRc<PollerInfo>),
    Worker(Worker),
    Timer(Pid, TimerType),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimerType {
    Real,
    Virtual,
    Prof
}

#[derive(Clone)]
pub enum SignalDestination {
    Process(Pid),
    Thread(Pid, ThreadId),
}

#[derive(Clone, Debug)]
pub enum CreateCowSource {
    New(FilePath<256>, AccessMode),
    Existing(CowId),
}
