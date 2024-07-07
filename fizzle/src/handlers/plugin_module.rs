use crate::arena::ArenaKey; 

use fizzle_plugin::FizzlePluginObject;

pub use private::PluginModuleId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct PluginModuleId(usize);
}


impl ArenaKey for PluginModuleId {
    type Value = Box<dyn FizzlePluginObject>;
}

impl PluginModuleId {

}
