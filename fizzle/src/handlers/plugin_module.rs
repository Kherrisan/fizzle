use crate::arena::ArenaKey;

use fizzle_plugin::PluginObject;

pub use private::PluginId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PluginId(usize);
}

impl ArenaKey for PluginId {
    type Value = Box<dyn PluginObject>;
}

impl PluginId {}
