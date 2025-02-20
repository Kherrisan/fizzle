use std::{cell::RefCell, rc::Rc};

use crate::{GlobalList, GlobalRc, GlobalVec};

use fizzle_plugin::{IoEndpointVariant, PluginObject, StreamId};

use super::polled::PolledInfo;

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
    pub module: Rc<RefCell<dyn PluginObject>>,
    pub read_buf: GlobalList<GlobalVec<u8>>,
    pub read_idx: usize,
    pub read_polled: GlobalRc<PolledInfo>,
    pub write_buf: GlobalList<GlobalVec<u8>>,
    pub write_idx: usize,
    pub write_polled: GlobalRc<PolledInfo>,
}
