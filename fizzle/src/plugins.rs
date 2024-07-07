use crate::arena::{KeyedArena, Rc};
use crate::constants::FIZZLE_MAX_PLUGINS;
use crate::handlers::plugin_module::PluginModuleId;

use fizzle_plugin::{Context, FizzlePluginObject, IoEndpointVariant};

use crate::state::FizzState;

pub type PluginModules =
    KeyedArena<PluginModuleId, Box<dyn FizzlePluginObject>, FIZZLE_MAX_PLUGINS>;

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

pub struct PluginConfigEndpoint {
    pub endpoint_variant: IoEndpointVariant,
    pub is_per_round: bool,
    pub emulation_type: IoEmulationType,
    //    pub module_id: Option<PluginModuleId>,
    pub num_streams: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IoEmulationType {
    #[allow(unused)]
    Passthrough,
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual file.
    #[allow(unused)]
    Feedback,
    /// Uses the plugin specified by `PluginId` to decide `read()`/`write()` behavior.
    #[allow(unused)]
    Plugin(Rc<PluginModuleId>),
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
pub fn run_plugins(state: &mut FizzState) -> bool {
    let mut plugin_activated = false;

    let mut read = Vec::new();
    let mut write = Vec::new();

    // TODO: turn this into an iterator in the future
    for plugin_info in state.global.plugins.values() {
        let mut raise_read = false;
        let mut lower_write = false;

        let plugin_module_id = plugin_info.module_id.clone();
        let context = Context {
            endpoint: plugin_info.endpoint.clone(),
            stream_id: plugin_info.stream,
        };

        let plugin_module = state 
            .local
            .plugin_modules
            .as_mut()
            .unwrap()
            .get_mut(&plugin_module_id)
            .unwrap();

        let write_buf_id = plugin_info.write_buf.clone();
        let write_polled = plugin_info.write_polled.clone();
        let read_buf_id = plugin_info.read_buf.clone();
        let read_polled = plugin_info.read_polled.clone();

        // Check read end
        let write_buf = state.global.buffers.get_mut(&write_buf_id).unwrap();
        if plugin_module.can_read(&context) && !write_buf.is_empty() {
            log::debug!("plugin module context {:?} can be read", &context);
            plugin_activated = true;
            match plugin_module.read(write_buf.data(), &context) {
                Ok(0) => unimplemented!(),
                Err(_) => unimplemented!(),
                Ok(amount) => {
                    log::debug!("plugin module read {} bytes", amount);
                    write_buf.did_read(amount);
                    if write_buf.is_empty() {
                        lower_write = true;
                    }
                }
            }
        }

        // Check write end
        let read_buf = state.global.buffers.get_mut(&read_buf_id).unwrap();
        if plugin_module.can_write(&context) && !read_buf.is_full() {
            log::debug!("plugin module context {:?} can write", &context);
            plugin_activated = true;
            match plugin_module.write(read_buf.remaining_mut(), &context) {
                Ok(0) => unimplemented!(),
                Err(_) => unimplemented!(),
                Ok(amount) => {
                    log::debug!("plugin module wrote {} bytes", amount);
                    read_buf.did_write(amount);
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
