use crate::arena::{ArenaKey, Rc}; 

use fizzle_plugin::{IoEndpointVariant, StreamId};
pub use private::PluginId;

use super::buffer::BufferId;
use super::plugin_module::PluginModuleId;
use super::polled::PolledId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PluginId(usize);
}

// Runtime active plugin I/O information
#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub endpoint: IoEndpointVariant,
    pub stream: StreamId,
    /// The plugin module to read/write from.
    pub module_id: Rc<PluginModuleId>,
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_buf: Rc<BufferId>,
    pub write_polled: Rc<PolledId>,
}

impl ArenaKey for PluginId {
    type Value = PluginInfo;
}

impl PluginId {

}
