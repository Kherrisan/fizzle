use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

// TODO: can we pass through configuration options for specific streams? How would we go about
// doing that?? The problem is that a plugin can be defined in multiple I/O endpoints, so the
// configuration would need to differentiate based on I/O endpoint. BUT, `stream_id` is too opaque
// to convey to the plugin what I/O endpoint it's associated with. One hacky way to account for
// this would be to order configuration temporally, but that would run into issues if I/O streams
// have different orderings based on fuzzing input.
//
// So, what do we do? It would be best to enable plugins to be reused across I/O endpoints while
// also allowing plugin configuration to be passed through the main fizzle configuration file;
// re-defining plugiins for each configuration desired doesn't seem ergonomic. To do this, we
// need to make `stream_id` non-opaque enough to convey I/O information...
//
// The good news is that StreamId can remain opaque for now, and we can just redefine its contents
// later to include non-opaque info that we expose via method calls.

/// The specific protocol, I/O endpoint and stream that a plugin method is called for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Context {
    pub endpoint: IoEndpoint,
    pub stream_id: StreamId,
}

/// A unique identifier that corresponds to a single data stream in `Fizzle`.
///
/// This identifier enables multiple connections of the same stream type to be handled
/// by a single plugin instance, thereby allowing for shared state across streams when needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(usize);

impl From<usize> for StreamId {
    /// Creates a new StreamId from the given value.
    ///
    /// NOTE: this is an unstable API. This should NOT be used when developing plugins.
    #[inline]
    fn from(value: usize) -> Self {
        StreamId(value)
    }
}

impl From<StreamId> for usize {
    /// Creates a new `usize` from the given `StreamId`.
    ///
    /// NOTE: this is an unstable API. This should NOT be used when developing plugins.
    #[inline]
    fn from(value: StreamId) -> Self {
        value.0
    }
}

/*
/// A unique identifier that corresponds to a single I/O endpoint defined in the `Fizzle`
/// configuration file.
///
/// This identifier enables multiple connections of the same stream type to be handled
/// by a single plugin instance, thereby allowing for shared state across streams when needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IoEndpointId(usize);
*/

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IoEndpoint {
    /// Standard input/output (`stdin` and `stdout`)
    Stdio,
    /// A particular file.
    File(PathBuf),
    TcpServer(SocketAddr),
    TcpClient(SocketAddr),
    UdpServer(SocketAddr),
    UdpClient(SocketAddr),
    SctpServer(SocketAddr),
    SctpClient(SocketAddr),
}

/// An error that a plugin may return during calls to [`read()`](FizzlePluginObject::read) or
/// [`write()`](FizzlePluginObject::write).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginError {
    /// No data could be read from/written to the plugin.
    NotReady,
    /// An unexpected error occurred within the plugin.
    InternalError,
    // NOTE: replaced by a read/write that returns 0
    // /// The underlying data transport medium the plugin communicates on should be closed.
    // Disconnect,
}

/// A plugin to `Fizzle` that can be used to model any source of I/O for a program.
///
/// Plugins can model pseudorandom configuration files, mimic network service dependencies,
/// or even add structure- and protocol-awareness to otherwise arbitrary fuzzing inputs.
pub trait FizzlePlugin: FizzlePluginObject {
    /// Constructs an instance of this plugin, configured with `config`.
    fn new(config: HashMap<IoEndpoint, toml::Table>) -> Self;
}

/// The object-safe subset of methods that must be implemented for a [`FizzlePlugin`].
///
/// Each method includes a [`Context`] that indicates what protocol and stream the method is being
/// called for. An application being tested may open an I/O device multiple times, or a plugin may
/// be applied to multiple I/O endpoints within configuration, so the plugin must be able to
/// differentiate between different `stream_id` values within the context. The `protocol_id` field
/// of the context is meant for plugins that implement multiple protocols (e.g., to share state
/// between protocols), so it can be safely ignored for plugins that only have one [`FizzlePlugin`]
/// implementation.
pub trait FizzlePluginObject {
    /// Loads a source of entropy (e.g., fuzzing input) that the plugin may base its behavior on.
    ///
    /// A plugin must exhibit the same behavior and outputs across consecutive runs for a given
    /// entropy input to preserve deterministic behavior during fuzzing/dynamic analysis.
    fn load_entropy(&mut self, entropy: &[u8]);

    /// Reads data from the service the plugin is modelling.
    fn read(&mut self, buf: &mut [u8], ctx: &Context) -> Result<usize, PluginError>;

    /// Writes data to the service the plugin is modelling.
    fn write(&mut self, buf: &[u8], ctx: &Context) -> Result<usize, PluginError>;

    /// Indicates to Fizzle whether the plugin has data ready to be read or not.
    fn can_read(&self, ctx: &Context) -> bool;

    /// Indicates to Fizzle whether the plugin is ready to have data written to it or not.
    fn can_write(&self, ctx: &Context) -> bool;
}
