use crate::datagram::DatagramConfig;
use crate::network::fd_inheritance::{BindStrategy, BindTarget};
use crate::stream::StreamConfig;
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
///     max_connections: 100,
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     ..UnixStreamConfig::default().with_socket_path("/tmp/echo.sock".into())
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
            fallback_target: Some(BindTarget::Unix(fallback_path)),
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
            bind_strategy: Some(config.bind_strategy),
            service_name: config.service_name,
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
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     ..UnixDatagramConfig::default().with_socket_path("/tmp/echo_dgram.sock".into())
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UnixDatagramConfig {
    /// Binding strategy for socket creation (supports FD inheritance)
    pub bind_strategy: BindStrategy,
    /// Service name for FD inheritance lookup
    pub service_name: String,
    /// Receive buffer size (default 64 KiB); larger datagrams are truncated
    pub buffer_size: usize,
    /// Idle receive timeout
    pub read_timeout: Duration,
    /// Timeout for sending each reply
    pub write_timeout: Duration,
}

impl Default for UnixDatagramConfig {
    fn default() -> Self {
        Self {
            bind_strategy: BindStrategy::Bind(BindTarget::Unix(
                "/tmp/echosrv_datagram.sock".into(),
            )),
            service_name: "unix-datagram".to_string(),
            buffer_size: crate::datagram::DEFAULT_DATAGRAM_BUFFER_SIZE,
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
            fallback_target: Some(BindTarget::Unix(fallback_path)),
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
            bind_strategy: Some(config.bind_strategy),
            service_name: config.service_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn assert_bind_path(strategy: &BindStrategy, expected: &str) {
        match strategy {
            BindStrategy::Bind(BindTarget::Unix(path)) => assert_eq!(path.as_os_str(), expected),
            other => panic!("expected Bind(Unix({expected})), got {other:?}"),
        }
    }

    #[track_caller]
    fn assert_inherit_or_bind_path(strategy: &BindStrategy, expected: &str) {
        match strategy {
            BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: Some(BindTarget::Unix(path)),
            } => assert_eq!(path.as_os_str(), expected),
            other => panic!("expected InheritOrBind(Unix({expected})), got {other:?}"),
        }
    }

    #[test]
    fn stream_defaults() {
        let config = UnixStreamConfig::default();
        assert_bind_path(&config.bind_strategy, "/tmp/echosrv_stream.sock");
        assert_eq!(config.service_name, "unix-stream");
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.buffer_size, 1024);
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
    }

    #[test]
    fn datagram_defaults() {
        let config = UnixDatagramConfig::default();
        assert_bind_path(&config.bind_strategy, "/tmp/echosrv_datagram.sock");
        assert_eq!(config.service_name, "unix-datagram");
        assert_eq!(
            config.buffer_size,
            crate::datagram::DEFAULT_DATAGRAM_BUFFER_SIZE
        );
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
    }

    #[test]
    fn with_socket_path_and_fd_inheritance() {
        let stream = UnixStreamConfig::default().with_socket_path("/run/a.sock".into());
        assert_bind_path(&stream.bind_strategy, "/run/a.sock");
        let stream = stream.with_fd_inheritance("svc".into(), "/run/b.sock".into());
        assert_eq!(stream.service_name, "svc");
        assert_inherit_or_bind_path(&stream.bind_strategy, "/run/b.sock");

        let dgram = UnixDatagramConfig::default().with_socket_path("/run/c.sock".into());
        assert_bind_path(&dgram.bind_strategy, "/run/c.sock");
        let dgram = dgram.with_fd_inheritance("dsvc".into(), "/run/d.sock".into());
        assert_eq!(dgram.service_name, "dsvc");
        assert_inherit_or_bind_path(&dgram.bind_strategy, "/run/d.sock");
    }

    #[test]
    fn stream_from_preserves_every_field() {
        let config = UnixStreamConfig {
            max_connections: 2,
            buffer_size: 33,
            read_timeout: Duration::from_millis(5),
            write_timeout: Duration::from_millis(6),
            ..UnixStreamConfig::default().with_fd_inheritance("custom".into(), "/run/e.sock".into())
        };
        let stream: StreamConfig = config.into();
        assert_eq!(stream.max_connections, 2);
        assert_eq!(stream.buffer_size, 33);
        assert_eq!(stream.read_timeout, Duration::from_millis(5));
        assert_eq!(stream.write_timeout, Duration::from_millis(6));
        assert_eq!(stream.service_name, "custom");
        // The Unix strategy always overrides the (unused) network bind_addr.
        assert_inherit_or_bind_path(stream.bind_strategy.as_ref().unwrap(), "/run/e.sock");
        assert_inherit_or_bind_path(&stream.effective_bind_strategy(), "/run/e.sock");
        stream.validate().unwrap();
    }

    #[test]
    fn datagram_from_preserves_every_field() {
        let config = UnixDatagramConfig {
            buffer_size: 44,
            read_timeout: Duration::from_millis(7),
            write_timeout: Duration::from_millis(8),
            ..UnixDatagramConfig::default().with_socket_path("/run/f.sock".into())
        };
        let dgram: DatagramConfig = config.into();
        assert_eq!(dgram.buffer_size, 44);
        assert_eq!(dgram.read_timeout, Duration::from_millis(7));
        assert_eq!(dgram.write_timeout, Duration::from_millis(8));
        assert_eq!(dgram.service_name, "unix-datagram");
        assert_bind_path(&dgram.effective_bind_strategy(), "/run/f.sock");
        dgram.validate().unwrap();
    }

    #[test]
    fn zero_values_are_rejected_after_conversion() {
        let stream: StreamConfig = UnixStreamConfig {
            max_connections: 0,
            ..Default::default()
        }
        .into();
        assert!(stream.validate().is_err());

        let dgram: DatagramConfig = UnixDatagramConfig {
            buffer_size: 0,
            ..Default::default()
        }
        .into();
        assert!(dgram.validate().is_err());
    }
}
