use crate::arena::ArenaKey; 

use fizzle_common::io::MAX_PATH_LEN;
use fizzle_common::path::FilePath;

pub use private::DirectoryId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct DirectoryId(usize);
}


impl ArenaKey for DirectoryId {
    type Value = FilePath<MAX_PATH_LEN>;
}

impl DirectoryId {

}
