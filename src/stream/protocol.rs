use super::config::StreamConfig;
use crate::network::fd_inheritance::FdInheritanceConfig;
use crate::network::{Address, LocalAddress};
use async_trait::async_trait;
use std::net::SocketAddr;

/// Trait for stream-based protocols (TCP, Unix streams, etc.)
///
/// This trait defines the interface that stream protocol implementations
/// must provide to work with the generic stream echo server and client.
///
/// File descriptor inheritance support is provided through optional methods
/// that protocols can implement for zero-downtime server reloads.
#[async_trait]
pub trait StreamProtocol: Send + Sync + 'static {
    /// Error type for this protocol
    type Error: Send + Into<crate::EchoError>;
    /// Listener type for this protocol
    type Listener: Send + LocalAddress;
    /// Stream type for this protocol
    type Stream: Send;

    /// Binds a listener to the given configuration (server-side)
    ///
    /// Implementations should honor [`StreamConfig::bind_strategy`] and use the
    /// process-wide systemd descriptor pool
    /// ([`FdInheritanceConfig::from_systemd_env`]) for service-name lookups.
    async fn bind(config: &StreamConfig) -> std::result::Result<Self::Listener, Self::Error>;

    /// Binds a listener with explicit file descriptor inheritance configuration
    ///
    /// Generic servers call this method. `fd_config` is the pool used for
    /// service-name lookups when the strategy is
    /// [`InheritOrBind`](crate::network::BindStrategy::InheritOrBind).
    ///
    /// The default implementation ignores `fd_config` and calls [`bind`](Self::bind).
    async fn bind_with_inheritance(
        config: &StreamConfig,
        _fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Listener, Self::Error> {
        // Default implementation ignores FD inheritance and uses standard binding
        // Protocols that support inheritance should override this method
        Self::bind(config).await
    }

    /// Accepts a new connection from the listener (server-side)
    async fn accept(
        listener: &mut Self::Listener,
    ) -> std::result::Result<(Self::Stream, SocketAddr), Self::Error>;

    /// Connects to a server at the given address (client-side)
    async fn connect(addr: SocketAddr) -> std::result::Result<Self::Stream, Self::Error>;

    /// Connects to a server at a unified [`Address`] (client-side).
    ///
    /// The default implementation handles [`Address::Network`] via
    /// [`connect`](Self::connect) and rejects [`Address::Unix`] with an
    /// [`Unsupported`](std::io::ErrorKind::Unsupported) I/O error. Unix
    /// protocols override it.
    async fn connect_address(addr: &Address) -> std::result::Result<Self::Stream, Self::Error> {
        match addr {
            Address::Network(addr) => Self::connect(*addr).await,
            Address::Unix(path) => Err(Self::map_io_error(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "protocol does not support Unix socket address {}",
                    path.display()
                ),
            ))),
        }
    }

    /// Reads data from a stream
    async fn read(
        stream: &mut Self::Stream,
        buffer: &mut [u8],
    ) -> std::result::Result<usize, Self::Error>;

    /// Writes data to a stream
    async fn write(stream: &mut Self::Stream, data: &[u8]) -> std::result::Result<(), Self::Error>;

    /// Flushes a stream
    async fn flush(stream: &mut Self::Stream) -> std::result::Result<(), Self::Error>;

    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;
}
