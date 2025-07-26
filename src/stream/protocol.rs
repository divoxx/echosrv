use super::config::StreamConfig;
use std::net::SocketAddr;
use std::future::Future;

/// Trait for stream-based protocols (TCP, Unix streams, etc.)
///
/// This trait defines the interface that stream protocol implementations
/// must provide to work with the generic stream echo server and client.
#[allow(async_fn_in_trait)]
pub trait StreamProtocol {
    /// Error type for this protocol
    type Error: Send + Into<crate::EchoError>;
    /// Listener type for this protocol
    type Listener: Send;
    /// Stream type for this protocol
    type Stream: Send;
    
    /// Binds a listener to the given configuration (server-side)
    fn bind(config: &StreamConfig) -> impl Future<Output = std::result::Result<Self::Listener, Self::Error>> + Send;
    
    /// Accepts a new connection from the listener (server-side)
    fn accept(listener: &mut Self::Listener) -> impl Future<Output = std::result::Result<(Self::Stream, SocketAddr), Self::Error>> + Send;
    
    /// Connects to a server at the given address (client-side)
    async fn connect(addr: SocketAddr) -> std::result::Result<Self::Stream, Self::Error>;
    
    /// Reads data from a stream
    fn read(stream: &mut Self::Stream, buffer: &mut [u8]) -> impl Future<Output = std::result::Result<usize, Self::Error>> + Send;
    
    /// Writes data to a stream
    fn write(stream: &mut Self::Stream, data: &[u8]) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send;
    
    /// Flushes a stream
    fn flush(stream: &mut Self::Stream) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send;
    
    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;
} 