use crate::network::{BindStrategy, BindTarget};
use crate::stream::StreamConfig;
use std::net::SocketAddr;
use std::time::Duration;

/// TCP-specific configuration that extends the common config
///
/// # Examples
///
/// ```
/// use echosrv::tcp::TcpConfig;
/// use std::time::Duration;
///
/// let config = TcpConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     max_connections: 100,
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     ..Default::default()
/// };
///
/// // Prefer a socket passed by systemd (FileDescriptorName=tcp), else bind.
/// let activated = config.with_fd_inheritance("tcp");
/// ```
#[derive(Debug, Clone)]
pub struct TcpConfig {
    /// Address to bind the server to (when not inheriting)
    pub bind_addr: SocketAddr,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
    /// Socket acquisition strategy; `None` binds `bind_addr`.
    /// See [`StreamConfig::bind_strategy`].
    pub bind_strategy: Option<BindStrategy>,
    /// Service name for inherited-descriptor lookup (default `"tcp"`).
    pub service_name: String,
}

impl Default for TcpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            bind_strategy: None,
            service_name: "tcp".to_string(),
        }
    }
}

impl TcpConfig {
    /// Prefer an inherited descriptor named `service_name` (e.g. systemd
    /// `FileDescriptorName=`), falling back to binding `bind_addr`.
    pub fn with_fd_inheritance(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self.bind_strategy = Some(BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: BindTarget::Network(self.bind_addr),
        });
        self
    }
}

impl From<TcpConfig> for StreamConfig {
    fn from(config: TcpConfig) -> Self {
        Self {
            bind_addr: config.bind_addr,
            max_connections: config.max_connections,
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            bind_strategy: config.bind_strategy,
            service_name: config.service_name,
        }
    }
}
