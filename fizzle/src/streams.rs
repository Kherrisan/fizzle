//! Process I/O shims.
//!
//!

use std::io;
use std::os::fd::RawFd;

#[allow(unused)]
pub trait Stream {
    /// Pass data into the stream.
    ///
    /// This method is used to handle `write` operations on the file descriptor being shimmed.
    fn input(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Retrieve data from the stream.
    ///
    /// This method is used to handle `read` operations on the file descriptor being shimmed.
    fn output(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Indicates whether the stream is ready to receive input data.
    ///
    /// This method is used to handle `write` (e.g. `POLLOUT`) polling events.
    fn ready_for_input(&self) -> bool;

    /// Indicates whether the stream has any output data ready.
    ///
    /// This method is used to handle `read` (e.g. `POLLIN`) polling events.
    fn ready_for_output(&self) -> bool;

    /// Retrieves any underlying file descriptor from the stream, if applicable.
    ///
    /// This method will only return `Some` if the `Stream` passes data through to a real file
    /// descriptor rather than simulating a connection.
    fn raw_fd(&self) -> Option<RawFd>;
}

// How do we map string inputs from config to implementations of this trait?
// Fundamentally a config parser problem: the parser has to be implemented
// by the lib that derives this crate
