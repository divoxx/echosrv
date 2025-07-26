use super::config::DatagramConfig;
use std::net::SocketAddr;
use std::future::Future;

/// Trait for datagram-based protocols (UDP, Unix datagrams, etc.)
///
/// This trait defines the interface that datagram protocol implementations
/// must provide to work with the generic datagram echo server.
pub trait DatagramProtocol {
    /// Error type for this protocol
    type Error: Send + Into<crate::EchoError>;
    /// Socket type for this protocol
    type Socket: Send;
    
    /// Binds a socket to the given configuration
    async fn bind(config: &DatagramConfig) -> std::result::Result<Self::Socket, Self::Error>;
    
    /// Receives data from a socket
    async fn recv_from(socket: &Self::Socket, buffer: &mut [u8]) -> std::result::Result<(usize, SocketAddr), Self::Error>;
    
    /// Sends data to a specific address
    async fn send_to(socket: &Self::Socket, data: &[u8], addr: SocketAddr) -> std::result::Result<usize, Self::Error>;
    
    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;
} 