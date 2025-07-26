use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for datagram-based echo servers
///
/// This struct contains all the configuration options needed for
/// datagram-based echo servers (UDP, Unix datagrams, etc.).
///
/// # Examples
///
/// ```
/// use echosrv::datagram::DatagramConfig;
/// use std::time::Duration;
///
/// let config = DatagramConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct DatagramConfig {
    /// Address to bind the server to
    pub bind_addr: SocketAddr,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for datagrams
    pub read_timeout: Duration,
    /// Write timeout for datagrams
    pub write_timeout: Duration,
}

impl Default for DatagramConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
} 