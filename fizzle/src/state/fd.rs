use crate::FilePath;

#[derive(Debug)]
pub enum FdInfo {
    /// Files `open`ed using O_PATH
    Directory(FilePath),
    /// Files that are accessed via the virtual filesystem.
    File(FilePath),
    /// Files that are accessed normally.
    PassthroughFile(FilePath),
}
