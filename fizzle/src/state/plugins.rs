use crate::constants::*;

use fizzle_common::io::IoEndpoint;
use fizzle_common::storage::ValueIndex;
use fizzle_plugin::{FizzlePluginObject, IoEndpointId};
use heapless::FnvIndexMap;

pub type PluginEndpoints = ValueIndex<IoEndpointId, IoEmulationType, FIZZLE_MAX_PLUGINS>;
pub type PluginModules = ValueIndex<PluginId, Box<dyn FizzlePluginObject>, FIZZLE_MAX_PLUGINS>;

pub type PluginMappings = FnvIndexMap<IoEndpoint, IoEndpointId, FIZZLE_MAX_PLUGINS>;

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
    pub endpoints: PluginEndpoints,
    pub modules: PluginModules,
    pub mappings: PluginMappings,
}

impl PluginConfig {
    pub fn new() -> Self {
        Self {
            endpoints: Default::default(),
            modules: Default::default(),
            mappings: FnvIndexMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginId(usize);

impl From<usize> for PluginId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Into<usize> for PluginId {
    fn into(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IoEmulationType {
    /// `read()`s will return whatever was written by prior `write()`s--acts as a virtual file.
    Feedback,
    /// Uses the plugin specified by `PluginId` to decide `read()`/`write()` behavior.
    Plugin(PluginId),
    Sink,
    NullSink,
    Fuzz,
    // TODO: add Passthrough here?
}
