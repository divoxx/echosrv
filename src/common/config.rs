use std::net::SocketAddr;
use std::time::Duration;

/// Common configuration for echo servers
///
/// This configuration is shared between TCP and UDP echo servers,
/// containing the common parameters needed for both protocols.
///
/// # Examples
///
/// ```
/// use echosrv::common::Config;
/// use std::time::Duration;
///
/// let config = Config {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
/// };
/// ```
///
/// Using the default configuration:
///
/// ```
/// use echosrv::common::Config;
///
/// let config = Config::default();
/// assert_eq!(config.buffer_size, 1024);
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind the server to
    pub bind_addr: SocketAddr,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(), // Use port 0 for testing
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
} 