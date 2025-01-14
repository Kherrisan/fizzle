use std::cell::RefCell;

use crate::constants::FIZZLE_BUFFER_LENGTH;
use crate::GlobalRc;

use fizzle_common::storage::Buffer;
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
    pub module: std::rc::Rc<RefCell<dyn PluginObject>>,
    pub read_buf: GlobalRc<Buffer<FIZZLE_BUFFER_LENGTH>>,
    pub read_polled: GlobalRc<PolledInfo>,
    pub write_buf: GlobalRc<Buffer<FIZZLE_BUFFER_LENGTH>>,
    pub write_polled: GlobalRc<PolledInfo>,
}

