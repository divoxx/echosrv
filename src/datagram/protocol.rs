use super::config::DatagramConfig;
use crate::network::LocalAddress;
use crate::network::fd_inheritance::FdInheritanceConfig;
use async_trait::async_trait;

/// Trait for datagram-based protocols (UDP, Unix datagrams, etc.)
///
/// This trait defines the interface that datagram protocol implementations
/// must provide to work with the generic datagram echo server.
#[async_trait]
pub trait DatagramProtocol: Send + Sync + 'static {
    /// Error type for this protocol
    type Error: Send + Into<crate::EchoError>;
    /// Socket type for this protocol
    type Socket: Send + Sync + LocalAddress;
    /// Peer address type (e.g. `SocketAddr` for UDP, a socket path for Unix
    /// datagrams). Replies are sent to the address returned by `recv_from`.
    type PeerAddr: Send + Sync + std::fmt::Debug + 'static;

    /// Binds a socket to the given configuration
    ///
    /// Implementations should honor [`DatagramConfig::bind_strategy`] and use
    /// the process-wide systemd descriptor pool for service-name lookups.
    async fn bind(config: &DatagramConfig) -> std::result::Result<Self::Socket, Self::Error>;

    /// Binds a socket with explicit file descriptor inheritance configuration
    ///
    /// Generic servers call this method. The default implementation ignores
    /// `fd_config` and calls [`bind`](Self::bind).
    async fn bind_with_inheritance(
        config: &DatagramConfig,
        _fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Socket, Self::Error> {
        Self::bind(config).await
    }

    /// Receives one datagram, returning its length and the sender's address
    async fn recv_from(
        socket: &Self::Socket,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, Self::PeerAddr), Self::Error>;

    /// Sends one datagram to `addr`
    async fn send_to(
        socket: &Self::Socket,
        data: &[u8],
        addr: &Self::PeerAddr,
    ) -> std::result::Result<usize, Self::Error>;

    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;
}
