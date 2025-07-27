use crate::datagram::DatagramConfig;
use crate::stream::StreamConfig;
use crate::network::fd_inheritance::{BindStrategy, BindTarget};
use std::path::PathBuf;
use std::time::Duration;

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
    /// Binding strategy for socket creation (supports FD inheritance)
    pub bind_strategy: BindStrategy,
    /// Service name for FD inheritance lookup
    pub service_name: String,
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
            bind_strategy: BindStrategy::Bind(BindTarget::Unix("/tmp/echosrv_stream.sock".into())),
            service_name: "unix-stream".to_string(),
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
}

impl UnixStreamConfig {
    /// Create configuration with specific socket path
    pub fn with_socket_path(mut self, path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::Bind(BindTarget::Unix(path));
        self
    }
    
    /// Enable FD inheritance with fallback to socket path
    pub fn with_fd_inheritance(mut self, service_name: String, fallback_path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: BindTarget::Unix(fallback_path),
        };
        self.service_name = service_name;
        self
    }
}

impl From<UnixStreamConfig> for StreamConfig {
    fn from(config: UnixStreamConfig) -> Self {
        Self {
            bind_addr: std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0,
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
    /// Binding strategy for socket creation (supports FD inheritance)
    pub bind_strategy: BindStrategy,
    /// Service name for FD inheritance lookup
    pub service_name: String,
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
            bind_strategy: BindStrategy::Bind(BindTarget::Unix("/tmp/echosrv_datagram.sock".into())),
            service_name: "unix-datagram".to_string(),
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
}

impl UnixDatagramConfig {
    /// Create configuration with specific socket path
    pub fn with_socket_path(mut self, path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::Bind(BindTarget::Unix(path));
        self
    }
    
    /// Enable FD inheritance with fallback to socket path
    pub fn with_fd_inheritance(mut self, service_name: String, fallback_path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: BindTarget::Unix(fallback_path),
        };
        self.service_name = service_name;
        self
    }
}

impl From<UnixDatagramConfig> for DatagramConfig {
    fn from(config: UnixDatagramConfig) -> Self {
        Self {
            bind_addr: std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0,
            ),
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
        }
    }
}
