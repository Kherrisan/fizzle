use std::cell::RefCell;

use crate::arena::{ArenaKey, Rc};

use fizzle_plugin::{IoEndpointVariant, PluginObject, StreamId};

use super::buffer::BufferId;
use super::polled::PolledId;

pub use private::PluginEndpointId;

// This is to forbid access to the PluginId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PluginEndpointId(usize);
}

// Runtime active plugin I/O information
#[derive(Clone)]
pub struct PluginInfo {
    pub endpoint: IoEndpointVariant,
    pub stream: StreamId,
    /// The plugin module to read/write from.
    pub module: std::rc::Rc<RefCell<dyn PluginObject>>,
    pub read_buf: Rc<BufferId>,
    pub read_polled: Rc<PolledId>,
    pub write_buf: Rc<BufferId>,
    pub write_polled: Rc<PolledId>,
}

impl ArenaKey for PluginEndpointId {
    type Value = PluginInfo;
}
