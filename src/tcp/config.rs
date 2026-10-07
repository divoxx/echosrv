use crate::network::BindStrategy;
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
    ///
    /// The fallback address is read when the server binds, so `bind_addr`
    /// may still be changed afterwards.
    pub fn with_fd_inheritance(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self.bind_strategy = Some(BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BindTarget;

    #[test]
    fn from_preserves_every_field() {
        let config = TcpConfig {
            bind_addr: "0.0.0.0:7000".parse().unwrap(),
            max_connections: 3,
            buffer_size: 77,
            read_timeout: Duration::from_millis(123),
            write_timeout: Duration::from_millis(456),
            bind_strategy: Some(BindStrategy::Bind(BindTarget::Network(
                "127.0.0.1:7001".parse().unwrap(),
            ))),
            service_name: "custom".into(),
        };
        let stream: StreamConfig = config.into();
        assert_eq!(stream.bind_addr, "0.0.0.0:7000".parse().unwrap());
        assert_eq!(stream.max_connections, 3);
        assert_eq!(stream.buffer_size, 77);
        assert_eq!(stream.read_timeout, Duration::from_millis(123));
        assert_eq!(stream.write_timeout, Duration::from_millis(456));
        assert_eq!(stream.service_name, "custom");
        match stream.bind_strategy {
            Some(BindStrategy::Bind(BindTarget::Network(addr))) => {
                assert_eq!(addr, "127.0.0.1:7001".parse().unwrap())
            }
            other => panic!("strategy not preserved: {other:?}"),
        }
    }

    #[test]
    fn default_converts_to_valid_stream_config() {
        let stream: StreamConfig = TcpConfig::default().into();
        assert_eq!(stream.service_name, "tcp");
        assert!(stream.bind_strategy.is_none());
        stream.validate().unwrap();
    }

    #[test]
    fn with_fd_inheritance_falls_back_to_current_bind_addr() {
        let mut config = TcpConfig::default().with_fd_inheritance("svc");
        // Changing bind_addr after enabling inheritance must take effect.
        config.bind_addr = "127.0.0.1:4321".parse().unwrap();
        let converted: StreamConfig = config.into();
        match converted.effective_bind_strategy() {
            BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: Some(BindTarget::Network(addr)),
            } => assert_eq!(addr, "127.0.0.1:4321".parse().unwrap()),
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn with_fd_inheritance() {
        let config = TcpConfig {
            bind_addr: "0.0.0.0:8080".parse().unwrap(),
            ..Default::default()
        }
        .with_fd_inheritance("web");
        assert_eq!(config.service_name, "web");
        match &config.bind_strategy {
            Some(BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: None,
            }) => {}
            other => panic!("unexpected strategy {other:?}"),
        }
        let stream: StreamConfig = config.into();
        assert_eq!(stream.service_name, "web");
        assert!(matches!(
            stream.effective_bind_strategy(),
            BindStrategy::InheritOrBind { .. }
        ));
    }
}
