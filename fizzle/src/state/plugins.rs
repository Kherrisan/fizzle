use crate::constants::*;

use fizzle_common::io::IoLocation;
use fizzle_common::storage::ValueIndex;
use fizzle_plugin::{FizzlePluginObject, IoLocationId};
use heapless::FnvIndexMap;

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
///
pub struct Plugins {
    pub plugins: ValueIndex<IoLocationId, Box<dyn FizzlePluginObject>, FIZZLE_MAX_PLUGINS>,
    pub io_mapping: FnvIndexMap<IoLocation, IoLocationId, FIZZLE_MAX_PLUGINS>,
}

impl Plugins {
    pub fn new() -> Self {
        Self {
            plugins: Default::default(),
            io_mapping: FnvIndexMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PluginId(usize);

impl PluginId {
    fn new(idx: usize) -> Self {
        Self(idx)
    }
}
