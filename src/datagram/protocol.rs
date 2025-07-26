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
    fn bind(config: &DatagramConfig) -> impl Future<Output = std::result::Result<Self::Socket, Self::Error>> + Send;
    
    /// Receives data from a socket
    fn recv_from(socket: &Self::Socket, buffer: &mut [u8]) -> impl Future<Output = std::result::Result<(usize, SocketAddr), Self::Error>> + Send;
    
    /// Sends data to a specific address
    fn send_to(socket: &Self::Socket, data: &[u8], addr: SocketAddr) -> impl Future<Output = std::result::Result<usize, Self::Error>> + Send;
    
    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;
} 