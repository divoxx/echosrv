use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for stream-based echo servers
///
/// This struct contains all the configuration options needed for
/// stream-based echo servers (TCP, Unix streams, etc.).
///
/// # Examples
///
/// ```
/// use echosrv::stream::StreamConfig;
/// use std::time::Duration;
///
/// let config = StreamConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     max_connections: 100,
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Address to bind the server to
    pub bind_addr: SocketAddr,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
} 