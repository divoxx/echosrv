use std::net::SocketAddr;
use std::time::Duration;
use std::future::Future;

/// Common configuration trait for echo servers
pub trait EchoConfig {
    fn bind_addr(&self) -> SocketAddr;
    fn buffer_size(&self) -> usize;
    fn read_timeout(&self) -> Duration;
    fn write_timeout(&self) -> Duration;
    fn max_connections(&self) -> usize { 1000 }
}

/// Stream-based protocols (TCP, Unix streams, etc.)
pub trait StreamProtocol {
    type Config: EchoConfig;
    type Error: Send;
    type Listener: Send;
    type Stream: Send;
    
    fn bind(config: &Self::Config) -> impl Future<Output = std::result::Result<Self::Listener, Self::Error>> + Send;
    fn accept(listener: &mut Self::Listener) -> impl Future<Output = std::result::Result<(Self::Stream, SocketAddr), Self::Error>> + Send;
    fn read(stream: &mut Self::Stream, buffer: &mut [u8]) -> impl Future<Output = std::result::Result<usize, Self::Error>> + Send;
    fn write(stream: &mut Self::Stream, data: &[u8]) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send;
    fn flush(stream: &mut Self::Stream) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send;
    
    fn map_io_error(err: std::io::Error) -> Self::Error;
}

/// Datagram-based protocols (UDP, Unix datagrams, etc.)
pub trait DatagramProtocol {
    type Config: EchoConfig;
    type Error: Send;
    type Socket: Send;
    
    fn bind(config: &Self::Config) -> impl Future<Output = std::result::Result<Self::Socket, Self::Error>> + Send;
    fn recv_from(socket: &Self::Socket, buffer: &mut [u8]) -> impl Future<Output = std::result::Result<(usize, SocketAddr), Self::Error>> + Send;
    fn send_to(socket: &Self::Socket, data: &[u8], addr: SocketAddr) -> impl Future<Output = std::result::Result<usize, Self::Error>> + Send;
    
    fn map_io_error(err: std::io::Error) -> Self::Error;
} 