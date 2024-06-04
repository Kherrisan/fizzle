use crate::constants::*;

use fizzle_common::storage::ValueIndex;
use fizzle_plugin::{Context, FizzlePluginObject, IoEndpointVariant};

use super::{FizzleContext, PluginId, PluginModuleId};

pub type PluginModules =
    ValueIndex<PluginModuleId, Box<dyn FizzlePluginObject>, FIZZLE_MAX_PLUGINS>;

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
pub struct PluginConfig {
    pub endpoints: Vec<PluginConfigEndpoint>,
    pub modules: PluginModules,
}

pub struct PluginConfigEndpoint {
    pub endpoint_variant: IoEndpointVariant,
    pub emulation_type: IoEmulationType,
//    pub module_id: Option<PluginModuleId>,
    pub num_streams: usize,
}

impl PluginConfig {
    pub fn new() -> Self {
        Self {
            endpoints: Default::default(),
            modules: Default::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IoEmulationType {
    #[allow(unused)]
    Passthrough,
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual file.
    #[allow(unused)]
    Feedback,
    /// Uses the plugin specified by `PluginId` to decide `read()`/`write()` behavior.
    #[allow(unused)]
    Plugin(PluginModuleId),
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
pub fn run_plugins(ctx: &mut FizzleContext) -> bool {
    let max_id = ctx.global().plugins.max_key();
    let mut plugin_activated = false;

    // TODO: turn this into an iterator in the future
    for i in 0..=max_id {
        // TODO: this is reeeeally messy... but it gets the job done
        // I would like to refactor to improve encapsulation of state more generally in the future
        let global = unsafe { &mut (*FizzleContext::interprocess_state(ctx.shared_memory)) };

        // TODO: handle datagrams here

        let plugin_id = PluginId::from(i);
        if let Some(plugin_info) = global.plugins.get(plugin_id) {
            let mut raise_read = false;
            let mut raise_write = false;

            let plugin_module_id = plugin_info.module_id;
            let context = Context {
                endpoint: plugin_info.endpoint.clone(),
                stream_id: plugin_info.stream.clone(),
            };

            let plugin_module = ctx.process_state.as_mut().plugin_modules.as_mut().unwrap().get_mut(plugin_module_id).unwrap();
            let write_buf_id = plugin_info.write_buf;
            let write_polled = plugin_info.write_polled;
            let read_buf_id = plugin_info.read_buf;
            let read_polled = plugin_info.read_polled;

            // Check read end
            let write_buf = global.buffers.get_mut(write_buf_id).unwrap();
            if plugin_module.can_read(&context) && !write_buf.is_empty() {
                plugin_activated = true;
                match plugin_module.read(write_buf.data(), &context) {
                    Ok(0) => unimplemented!(),
                    Err(_) => unimplemented!(),
                    Ok(amount) => {
                        write_buf.did_read(amount);
                        raise_write = true;
                    }
                }
            }

            // Check write end
            let read_buf = global.buffers.get_mut(read_buf_id).unwrap();
            if plugin_module.can_write(&context) && !read_buf.is_full() {
                plugin_activated = true;
                match plugin_module.write(read_buf.remaining_mut(), &context) {
                    Ok(0) => unimplemented!(),
                    Err(_) => unimplemented!(),
                    Ok(amount) => {
                        read_buf.did_write(amount);
                        raise_read = true;
                    }
                }
            }

            if raise_read {
                ctx.raise_polled(read_polled);
            }
            if raise_write {
                ctx.raise_polled(write_polled);
            }
        }
    }

    plugin_activated
}
