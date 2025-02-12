use std::cell::RefCell;

use fizzle_plugin::{Context, IoEndpointVariant, PluginObject};

use crate::state::FizzleState;

/// Plugin information, populated based on the Fizzle configuration file.
///
/// This is held in a special process-local area (despite its purposes being process-global).
/// The reason for this is that we want plugins to be able to make arbitrary heap allocations;
/// shared global state has to be both `Sized` and heapless, which would severely restrict plugin
/// functionality. We want to be able to reuse any cargo crates that would be useful for
/// structuring, so this seems like the best compromise.
///
/// The consequence of plugins being process-local is that they can only be called from that
/// process :/. This doesn't affect anything for single-process execution, but for multi-process
/// systems we'll need to switch back execution to the process containing the plugins whenever
/// there's new input available for them.
///
/// We can keep plugin data in the first process spawned, as most fuzzers assume that if the main
/// process exits then a crash has occurred. Fizzle has a special FIZZLE_NOEXIT option that can be
/// set to keep the main process alive after a call to `exit()`

pub struct PluginEndpoint {
    pub endpoint_variant: IoEndpointVariant,
    pub is_per_round: bool,
    pub emulation_type: IoEmulationType,
    pub num_streams: usize,
}

#[derive(Clone)]
pub enum IoEmulationType {
    #[allow(unused)]
    Passthrough,
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual file.
    #[allow(unused)]
    Feedback,
    /// Uses the plugin specified by the Rc to decide `read()`/`write()` behavior.
    #[allow(unused)]
    Plugin(std::rc::Rc<RefCell<dyn PluginObject>>),
    #[allow(unused)]
    Sink,
    #[allow(unused)]
    NullSink,
    #[allow(unused)]
    Fuzz,
}

/// Runs all plugins defined within the fizzle shim, returning `true` if any plugin raised events
/// that may require further handling by the process.
///
/// # Panics
///
/// This method will panic if it is not called in the root process
pub fn run_plugins(state: &mut FizzleState) -> bool {
    let mut plugin_activated = false;

    let mut read = Vec::new();
    let mut write = Vec::new();

    // TODO: turn this into an iterator in the future
    for plugin_info in state.global.plugins.iter() {
        let mut raise_read = false;
        let mut lower_write = false;

        let plugin_info_ref = plugin_info.borrow();

        let plugin_module = plugin_info_ref.module.clone();
        let context = Context {
            endpoint: plugin_info_ref.endpoint.clone(),
            stream_id: plugin_info_ref.stream,
        };

        let write_buf = plugin_info_ref.write_buf.clone();
        let write_polled = plugin_info_ref.write_polled.clone();
        let read_buf = plugin_info_ref.read_buf.clone();
        let read_polled = plugin_info_ref.read_polled.clone();

        // Check read end
        if plugin_module.borrow().can_read(&context) && !write_buf.borrow().is_empty() {
            log::debug!("plugin module context {:?} can be read", &context);
            plugin_activated = true;
            match plugin_module.borrow_mut().read(write_buf.borrow().data(), &context) {
                Ok(0) => unimplemented!(),
                Err(_) => unimplemented!(),
                Ok(amount) => {
                    log::debug!("plugin module read {} bytes", amount);
                    write_buf.borrow_mut().did_read(amount);
                    if write_buf.borrow().is_empty() {
                        lower_write = true;
                    }
                }
            }
        }

        let mut read_buf_mut = read_buf.borrow_mut();
        // Check write end
        if plugin_module.borrow().can_write(&context) && !read_buf_mut.is_full() {
            log::debug!("plugin module context {:?} can write", &context);
            plugin_activated = true;
            match plugin_module.borrow_mut().write(read_buf_mut.remaining_mut(), &context) {
                Ok(0) => unimplemented!(),
                Err(_) => unimplemented!(),
                Ok(amount) => {
                    log::debug!("plugin module wrote {} bytes", amount);
                    read_buf_mut.did_write(amount);
                    raise_read = true;
                }
            }
        }

        if raise_read {
            read.push(read_polled.clone());
        }
        if lower_write {
            write.push(write_polled.clone());
        }
    }

    for read_polled in read {
        state.raise_polled(&read_polled);
    }

    for write_polled in write {
        state.lower_polled(&write_polled);
    }

    plugin_activated
}
