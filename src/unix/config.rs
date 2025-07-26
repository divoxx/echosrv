use std::path::PathBuf;
use std::time::Duration;
use crate::stream::StreamConfig;
use crate::datagram::DatagramConfig;

/// Unix domain stream socket configuration
///
/// # Examples
///
/// ```
/// use echosrv::unix::UnixStreamConfig;
/// use std::time::Duration;
///
/// let config = UnixStreamConfig {
///     socket_path: "/tmp/echo.sock".into(),
///     max_connections: 100,
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UnixStreamConfig {
    /// Path to the Unix domain socket file
    pub socket_path: PathBuf,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
}

impl Default for UnixStreamConfig {
    fn default() -> Self {
        Self {
            socket_path: "/tmp/echosrv_stream.sock".into(),
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
}

impl From<UnixStreamConfig> for StreamConfig {
    fn from(config: UnixStreamConfig) -> Self {
        Self {
            bind_addr: std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0
            ),
            max_connections: config.max_connections,
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
        }
    }
}

/// Unix domain datagram socket configuration
///
/// # Examples
///
/// ```
/// use echosrv::unix::UnixDatagramConfig;
/// use std::time::Duration;
///
/// let config = UnixDatagramConfig {
///     socket_path: "/tmp/echo_dgram.sock".into(),
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UnixDatagramConfig {
    /// Path to the Unix domain socket file
    pub socket_path: PathBuf,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
}

impl Default for UnixDatagramConfig {
    fn default() -> Self {
        Self {
            socket_path: "/tmp/echosrv_datagram.sock".into(),
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
}

impl From<UnixDatagramConfig> for DatagramConfig {
    fn from(config: UnixDatagramConfig) -> Self {
        Self {
            bind_addr: std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0
            ),
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
        }
    }
} 