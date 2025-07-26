use std::net::SocketAddr;
use std::time::Duration;

/// UDP-specific configuration that extends the common config
///
/// # Examples
///
/// ```
/// use echosrv::udp::UdpConfig;
/// use std::time::Duration;
///
/// let config = UdpConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UdpConfig {
    /// Address to bind the server to
    pub bind_addr: SocketAddr,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
} 