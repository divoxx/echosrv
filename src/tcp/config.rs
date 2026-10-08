//! TCP server configuration.

use crate::network::BindStrategy;
use crate::rate_limit::RateLimitConfig;
use crate::stream::StreamConfig;
use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for [`TcpEchoServer`](crate::TcpEchoServer).
///
/// Converts into [`StreamConfig`] (field for field), which is what
/// `TcpEchoServer::new` takes. Defaults: `127.0.0.1:0`, 100 connections,
/// 1 KiB buffer, 30 s timeouts, service name `"tcp"`.
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
    /// Address to bind the server to (when not inheriting).
    pub bind_addr: SocketAddr,
    /// Maximum number of concurrent connections; further connections are
    /// accepted and closed immediately.
    pub max_connections: usize,
    /// Per-connection read/echo buffer size (must be non-zero).
    pub buffer_size: usize,
    /// How long a connection may be idle (no data) before it is closed.
    pub read_timeout: Duration,
    /// Timeout for echoing each chunk back; the connection is closed on expiry.
    pub write_timeout: Duration,
    /// Socket acquisition strategy; `None` binds `bind_addr`.
    /// See [`StreamConfig::bind_strategy`].
    pub bind_strategy: Option<BindStrategy>,
    /// Service name for inherited-descriptor lookup (default `"tcp"`).
    pub service_name: String,
    /// Request rate limit (`None`, the default, means unlimited).
    /// See [`StreamConfig::rate_limit`].
    pub rate_limit: Option<RateLimitConfig>,
    /// New-connection rate limit (`None`, the default, means unlimited).
    /// See [`StreamConfig::accept_rate_limit`].
    pub accept_rate_limit: Option<RateLimitConfig>,
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
            rate_limit: None,
            accept_rate_limit: None,
        }
    }
}

impl TcpConfig {
    /// Prefer an inherited descriptor named `service_name` (e.g. systemd
    /// `FileDescriptorName=`), or the only descriptor if exactly one was
    /// passed, falling back to binding `bind_addr`.
    ///
    /// Sets [`bind_strategy`](Self::bind_strategy) to
    /// [`BindStrategy::InheritOrBind`]; see [`take_named_or_sole`] for the
    /// lookup rule.
    ///
    /// [`take_named_or_sole`]: crate::network::FdInheritanceConfig::take_named_or_sole
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

    /// Limits the request rate (see [`StreamConfig::rate_limit`]).
    pub fn with_rate_limit(mut self, limit: RateLimitConfig) -> Self {
        self.rate_limit = Some(limit);
        self
    }

    /// Limits the new-connection rate (see
    /// [`StreamConfig::accept_rate_limit`]).
    pub fn with_accept_rate_limit(mut self, limit: RateLimitConfig) -> Self {
        self.accept_rate_limit = Some(limit);
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
            rate_limit: config.rate_limit,
            accept_rate_limit: config.accept_rate_limit,
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
            rate_limit: Some(RateLimitConfig::new(5, 6)),
            accept_rate_limit: Some(RateLimitConfig::new(7, 8)),
        };
        let stream: StreamConfig = config.into();
        assert_eq!(stream.bind_addr, "0.0.0.0:7000".parse().unwrap());
        assert_eq!(stream.max_connections, 3);
        assert_eq!(stream.buffer_size, 77);
        assert_eq!(stream.read_timeout, Duration::from_millis(123));
        assert_eq!(stream.write_timeout, Duration::from_millis(456));
        assert_eq!(stream.service_name, "custom");
        assert_eq!(stream.rate_limit, Some(RateLimitConfig::new(5, 6)));
        assert_eq!(stream.accept_rate_limit, Some(RateLimitConfig::new(7, 8)));
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
