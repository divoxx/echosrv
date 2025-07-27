use super::config::DatagramConfig;
use crate::network::fd_inheritance::FdInheritanceConfig;
use async_trait::async_trait;
use std::net::SocketAddr;

/// Trait for datagram-based protocols (UDP, Unix datagrams, etc.)
///
/// This trait defines the interface that datagram protocol implementations
/// must provide to work with the generic datagram echo server.
/// 
/// File descriptor inheritance support is provided through optional methods
/// that protocols can implement for zero-downtime server reloads.
#[async_trait]
pub trait DatagramProtocol {
    /// Error type for this protocol
    type Error: Send + Into<crate::EchoError>;
    /// Socket type for this protocol
    type Socket: Send;

    /// Binds a socket to the given configuration
    /// 
    /// This method provides backward compatibility and automatically detects
    /// file descriptor inheritance from the environment (e.g., systemd).
    async fn bind(config: &DatagramConfig) -> std::result::Result<Self::Socket, Self::Error>;

    /// Binds a socket with explicit file descriptor inheritance configuration
    /// 
    /// This method enables advanced control over FD inheritance for custom
    /// deployment scenarios or process managers that don't use standard
    /// environment variables.
    /// 
    /// Default implementation falls back to the standard bind() method for
    /// backward compatibility with existing protocol implementations.
    async fn bind_with_inheritance(
        config: &DatagramConfig,
        _fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Socket, Self::Error> {
        // Default implementation ignores FD inheritance and uses standard binding
        // Protocols that support inheritance should override this method
        Self::bind(config).await
    }

    /// Receives data from a socket
    async fn recv_from(
        socket: &Self::Socket,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, SocketAddr), Self::Error>;

    /// Sends data to a specific address
    async fn send_to(
        socket: &Self::Socket,
        data: &[u8],
        addr: SocketAddr,
    ) -> std::result::Result<usize, Self::Error>;

    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;
}
