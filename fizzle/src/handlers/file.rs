use crate::arena::ArenaKey;
use crate::constants::FIZZLE_FOPEN_BUFSIZE;
use crate::backend::FileBackend;

use super::descriptor::DescriptorId;

use fizzle_common::storage::Buffer;

pub use private::FileId;

// This is to forbid access to the SocketId's inner `usize` field.
mod private {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[repr(transparent)]
    pub struct FileId(usize);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilePtr(usize);

impl From<*mut libc::FILE> for FilePtr {
    fn from(value: *mut libc::FILE) -> Self {
        FilePtr(value as usize)
    }
}

#[derive(Debug)]
pub struct FileObject {
    pub descriptor_id: DescriptorId,
    pub buf: Buffer<FIZZLE_FOPEN_BUFSIZE>,
}

impl ArenaKey for FileId {
    type Value = FileBackend;
}

impl FileId {

}
