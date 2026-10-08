//! Unix domain stream and datagram server configuration.

use crate::datagram::DatagramConfig;
use crate::defaults::{DEFAULT_UNIX_DGRAM_PATH, DEFAULT_UNIX_STREAM_PATH};
use crate::network::fd_inheritance::{BindStrategy, BindTarget};
use crate::rate_limit::RateLimitConfig;
use crate::stream::StreamConfig;
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for [`UnixStreamEchoServer`](crate::UnixStreamEchoServer).
///
/// Unlike the network configs there is no `bind_addr`: the socket path is part
/// of [`bind_strategy`](Self::bind_strategy). Defaults: binds
/// `/tmp/echosrv_stream.sock`, 100 connections, 1 KiB buffer, 30 s timeouts,
/// service name `"unix-stream"`.
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
    /// How to obtain the listening socket: bind a path, inherit a
    /// descriptor, or both (see [`BindStrategy`]). Network targets are
    /// rejected with [`EchoError::Config`](crate::EchoError::Config).
    pub bind_strategy: BindStrategy,
    /// Service name used to look up an inherited descriptor with
    /// [`BindStrategy::InheritOrBind`].
    pub service_name: String,
    /// Maximum number of concurrent connections (must be non-zero); further
    /// connections are accepted and closed immediately.
    pub max_connections: usize,
    /// Per-connection read/echo buffer size (must be non-zero).
    pub buffer_size: usize,
    /// How long a connection may be idle (no data) before it is closed.
    pub read_timeout: Duration,
    /// Timeout for echoing each chunk back; the connection is closed on expiry.
    pub write_timeout: Duration,
    /// Request rate limit (`None`, the default, means unlimited). Every chunk
    /// read counts as one request; an over-limit connection is closed.
    /// See [`StreamConfig::rate_limit`].
    pub rate_limit: Option<RateLimitConfig>,
    /// New-connection rate limit (`None`, the default, means unlimited).
    /// Over-limit connections are closed right after accept.
    /// See [`StreamConfig::accept_rate_limit`].
    pub accept_rate_limit: Option<RateLimitConfig>,
}

impl Default for UnixStreamConfig {
    fn default() -> Self {
        Self {
            bind_strategy: BindStrategy::Bind(BindTarget::Unix(DEFAULT_UNIX_STREAM_PATH.into())),
            service_name: "unix-stream".to_string(),
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            rate_limit: None,
            accept_rate_limit: None,
        }
    }
}

impl UnixStreamConfig {
    /// Binds `path` (no inheritance): sets
    /// [`bind_strategy`](Self::bind_strategy) to
    /// [`BindStrategy::Bind`]`(`[`BindTarget::Unix`]`(path))`.
    pub fn with_socket_path(mut self, path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::Bind(BindTarget::Unix(path));
        self
    }

    /// Prefer an inherited descriptor named `service_name` (or the only
    /// descriptor if exactly one was passed), falling back to binding
    /// `fallback_path`.
    ///
    /// Sets [`bind_strategy`](Self::bind_strategy) to
    /// [`BindStrategy::InheritOrBind`] and
    /// [`service_name`](Self::service_name) to `service_name`.
    pub fn with_fd_inheritance(mut self, service_name: String, fallback_path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix(fallback_path)),
        };
        self.service_name = service_name;
        self
    }

    /// Limits the request rate (see [`rate_limit`](Self::rate_limit)).
    pub fn with_rate_limit(mut self, limit: RateLimitConfig) -> Self {
        self.rate_limit = Some(limit);
        self
    }

    /// Limits the new-connection rate (see
    /// [`accept_rate_limit`](Self::accept_rate_limit)).
    pub fn with_accept_rate_limit(mut self, limit: RateLimitConfig) -> Self {
        self.accept_rate_limit = Some(limit);
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
            rate_limit: config.rate_limit,
            accept_rate_limit: config.accept_rate_limit,
        }
    }
}

/// Configuration for [`UnixDatagramEchoServer`](crate::UnixDatagramEchoServer).
///
/// Defaults: binds `/tmp/echosrv_datagram.sock`, 64 KiB buffer, 30 s
/// timeouts, service name `"unix-datagram"`.
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
    /// How to obtain the socket: bind a path, inherit a descriptor, or both
    /// (see [`BindStrategy`]). Network targets are rejected with
    /// [`EchoError::Config`](crate::EchoError::Config).
    pub bind_strategy: BindStrategy,
    /// Service name used to look up an inherited descriptor with
    /// [`BindStrategy::InheritOrBind`].
    pub service_name: String,
    /// Receive buffer size (must be non-zero); larger datagrams are truncated.
    pub buffer_size: usize,
    /// Idle receive timeout. Expiry is not an error; the server keeps waiting.
    pub read_timeout: Duration,
    /// Timeout for sending each reply.
    pub write_timeout: Duration,
    /// Datagram rate limit (`None`, the default, means unlimited). Over-limit
    /// datagrams are dropped. See [`DatagramConfig::rate_limit`].
    pub rate_limit: Option<RateLimitConfig>,
}

impl Default for UnixDatagramConfig {
    fn default() -> Self {
        Self {
            bind_strategy: BindStrategy::Bind(BindTarget::Unix(DEFAULT_UNIX_DGRAM_PATH.into())),
            service_name: "unix-datagram".to_string(),
            buffer_size: crate::datagram::DEFAULT_DATAGRAM_BUFFER_SIZE,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            rate_limit: None,
        }
    }
}

impl UnixDatagramConfig {
    /// Binds `path` (no inheritance): sets
    /// [`bind_strategy`](Self::bind_strategy) to
    /// [`BindStrategy::Bind`]`(`[`BindTarget::Unix`]`(path))`.
    pub fn with_socket_path(mut self, path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::Bind(BindTarget::Unix(path));
        self
    }

    /// Prefer an inherited descriptor named `service_name` (or the only
    /// descriptor if exactly one was passed), falling back to binding
    /// `fallback_path`.
    ///
    /// Sets [`bind_strategy`](Self::bind_strategy) to
    /// [`BindStrategy::InheritOrBind`] and
    /// [`service_name`](Self::service_name) to `service_name`.
    pub fn with_fd_inheritance(mut self, service_name: String, fallback_path: PathBuf) -> Self {
        self.bind_strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix(fallback_path)),
        };
        self.service_name = service_name;
        self
    }

    /// Limits the datagram rate (see [`rate_limit`](Self::rate_limit)).
    pub fn with_rate_limit(mut self, limit: RateLimitConfig) -> Self {
        self.rate_limit = Some(limit);
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
            rate_limit: config.rate_limit,
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
            ..UnixStreamConfig::default()
                .with_fd_inheritance("custom".into(), "/run/e.sock".into())
                .with_rate_limit(RateLimitConfig::new(5, 6))
                .with_accept_rate_limit(RateLimitConfig::new(7, 8))
        };
        let stream: StreamConfig = config.into();
        assert_eq!(stream.max_connections, 2);
        assert_eq!(stream.buffer_size, 33);
        assert_eq!(stream.read_timeout, Duration::from_millis(5));
        assert_eq!(stream.write_timeout, Duration::from_millis(6));
        assert_eq!(stream.service_name, "custom");
        assert_eq!(stream.rate_limit, Some(RateLimitConfig::new(5, 6)));
        assert_eq!(stream.accept_rate_limit, Some(RateLimitConfig::new(7, 8)));
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
            ..UnixDatagramConfig::default()
                .with_socket_path("/run/f.sock".into())
                .with_rate_limit(RateLimitConfig::new(9, 10))
        };
        let dgram: DatagramConfig = config.into();
        assert_eq!(dgram.buffer_size, 44);
        assert_eq!(dgram.read_timeout, Duration::from_millis(7));
        assert_eq!(dgram.write_timeout, Duration::from_millis(8));
        assert_eq!(dgram.service_name, "unix-datagram");
        assert_eq!(dgram.rate_limit, Some(RateLimitConfig::new(9, 10)));
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
