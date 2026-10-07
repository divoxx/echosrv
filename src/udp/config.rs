use crate::datagram::{DEFAULT_DATAGRAM_BUFFER_SIZE, DatagramConfig};
use crate::network::{BindStrategy, BindTarget};
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
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UdpConfig {
    /// Address to bind the server to (when not inheriting)
    pub bind_addr: SocketAddr,
    /// Receive buffer size (default 64 KiB); larger datagrams are truncated
    pub buffer_size: usize,
    /// Idle receive timeout
    pub read_timeout: Duration,
    /// Timeout for sending each reply
    pub write_timeout: Duration,
    /// Socket acquisition strategy; `None` binds `bind_addr`.
    /// See [`DatagramConfig::bind_strategy`].
    pub bind_strategy: Option<BindStrategy>,
    /// Service name for inherited-descriptor lookup (default `"udp"`).
    pub service_name: String,
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            buffer_size: DEFAULT_DATAGRAM_BUFFER_SIZE,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            bind_strategy: None,
            service_name: "udp".to_string(),
        }
    }
}

impl UdpConfig {
    /// Prefer an inherited descriptor named `service_name`, falling back to
    /// binding `bind_addr`.
    pub fn with_fd_inheritance(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self.bind_strategy = Some(BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: BindTarget::Network(self.bind_addr),
        });
        self
    }
}

impl From<UdpConfig> for DatagramConfig {
    fn from(config: UdpConfig) -> Self {
        Self {
            bind_addr: config.bind_addr,
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            bind_strategy: config.bind_strategy,
            service_name: config.service_name,
        }
    }
}
