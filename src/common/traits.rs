use crate::{Result, EchoError};
use std::net::SocketAddr;

/// Common trait for echo servers
///
/// This trait defines the common interface that all echo servers
/// (TCP, UDP, etc.) must implement.
pub trait EchoServerTrait {
    /// Starts the echo server and listens for connections/messages
    async fn run(&self) -> Result<()>;

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()>;
}

/// Common trait for echo clients
///
/// This trait defines the common interface that all echo clients
/// (TCP, UDP, etc.) must implement.
pub trait EchoClient {
    /// Sends data to the echo server and returns the echoed response
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>>;

    /// Sends a string and returns the echoed string
    async fn echo_string(&mut self, data: &str) -> Result<String> {
        let response = self.echo(data.as_bytes()).await?;
        String::from_utf8(response).map_err(EchoError::Utf8)
    }
}

/// Common trait for echo server builders
///
/// This trait provides a common interface for creating echo servers
/// with different configurations.
pub trait EchoServerBuilder {
    type Config;
    type Server: EchoServerTrait;

    /// Creates a new echo server with the given configuration
    fn new(config: Self::Config) -> Self::Server;
}

/// Common trait for echo client builders
///
/// This trait provides a common interface for creating echo clients
/// that can connect to echo servers.
pub trait EchoClientBuilder {
    type Client: EchoClient;

    /// Connects to an echo server at the given address
    async fn connect(addr: SocketAddr) -> Result<Self::Client>;
} 