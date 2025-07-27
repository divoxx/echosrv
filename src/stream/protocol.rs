use super::config::StreamConfig;
use crate::network::fd_inheritance::FdInheritanceConfig;
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
pub trait StreamProtocol {
    /// Error type for this protocol
    type Error: Send + Into<crate::EchoError>;
    /// Listener type for this protocol
    type Listener: Send;
    /// Stream type for this protocol
    type Stream: Send;

    /// Binds a listener to the given configuration (server-side)
    /// 
    /// This method provides backward compatibility and automatically detects
    /// file descriptor inheritance from the environment (e.g., systemd).
    async fn bind(config: &StreamConfig) -> std::result::Result<Self::Listener, Self::Error>;

    /// Binds a listener with explicit file descriptor inheritance configuration
    /// 
    /// This method enables advanced control over FD inheritance for custom
    /// deployment scenarios or process managers that don't use standard
    /// environment variables.
    /// 
    /// Default implementation falls back to the standard bind() method for
    /// backward compatibility with existing protocol implementations.
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
