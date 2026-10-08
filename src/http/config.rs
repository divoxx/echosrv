//! Configuration for the HTTP echo server.

use super::protocol::DEFAULT_MAX_BODY_SIZE;
use crate::network::BindStrategy;
use crate::rate_limit::RateLimitConfig;
use crate::stream::StreamConfig;
use std::time::Duration;

/// Configuration for [`HttpEchoServer`](crate::http::HttpEchoServer).
///
/// The connection-level fields mirror [`StreamConfig`]. The remaining fields
/// control the HTTP responses.
///
/// # Examples
///
/// ```rust
/// use echosrv::http::HttpConfig;
/// use std::time::Duration;
///
/// let config = HttpConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     max_connections: 100,
///     buffer_size: 8192,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     server_name: Some("EchoServer/1.0".to_string()),
///     default_content_type: Some("text/plain".to_string()),
///     max_body_size: 1024 * 1024,
///     bind_strategy: None,
///     service_name: "http".to_string(),
///     rate_limit: None,
///     accept_rate_limit: None,
/// };
///
/// // Prefer a socket passed by systemd (FileDescriptorName=http), else bind.
/// let activated = config.clone().with_fd_inheritance("http");
/// assert!(activated.bind_strategy.is_some());
///
/// // Or start from the defaults:
/// let config = HttpConfig {
///     max_body_size: 64 * 1024,
///     ..HttpConfig::default()
/// };
/// assert_eq!(config.max_body_size, 64 * 1024);
/// ```
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Network address to bind to (when not inheriting a socket).
    pub bind_addr: std::net::SocketAddr,
    /// Maximum number of concurrent connections.
    pub max_connections: usize,
    /// Size of the per-connection buffer used to read and echo the body. It
    /// does not limit the body size (see `max_body_size`).
    pub buffer_size: usize,
    /// Read timeout for connections. It applies to each read, including
    /// waiting for the request head.
    pub read_timeout: Duration,
    /// Write timeout for connections.
    pub write_timeout: Duration,
    /// Value of the `Server` response header. `None` omits the header.
    pub server_name: Option<String>,
    /// Value of the `Content-Type` header on `200 OK` responses. `None` omits
    /// the header. The request's own `Content-Type` is not echoed.
    pub default_content_type: Option<String>,
    /// Largest accepted request body in bytes. Requests whose
    /// `Content-Length` exceeds it are answered with `413 Content Too Large`.
    pub max_body_size: usize,
    /// Socket acquisition strategy; `None` binds `bind_addr`.
    /// See [`StreamConfig::bind_strategy`].
    pub bind_strategy: Option<BindStrategy>,
    /// Service name for inherited-descriptor lookup (default `"http"`).
    /// See [`StreamConfig::service_name`].
    pub service_name: String,
    /// Request rate limit (`None`, the default, means unlimited). Over-limit
    /// requests, valid or not, get `429 Too Many Requests` with a
    /// `Retry-After` header.
    /// See [`StreamConfig::rate_limit`].
    pub rate_limit: Option<RateLimitConfig>,
    /// New-connection rate limit (`None`, the default, means unlimited).
    /// The request on an over-limit connection is read and answered with
    /// `429 Too Many Requests`. See [`StreamConfig::accept_rate_limit`].
    pub accept_rate_limit: Option<RateLimitConfig>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            max_connections: 100,
            buffer_size: 8192,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            server_name: Some("EchoServer/1.0".to_string()),
            default_content_type: Some("text/plain".to_string()),
            max_body_size: DEFAULT_MAX_BODY_SIZE,
            bind_strategy: None,
            service_name: "http".to_string(),
            rate_limit: None,
            accept_rate_limit: None,
        }
    }
}

impl HttpConfig {
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

/// Extracts the connection-level settings.
///
/// The HTTP-specific fields are not part of [`StreamConfig`]. They are applied
/// by [`HttpEchoServer::new`](crate::http::HttpEchoServer::new), so build the
/// server from an `HttpConfig` rather than from this conversion.
impl From<HttpConfig> for StreamConfig {
    fn from(config: HttpConfig) -> Self {
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
    fn defaults() {
        let config = HttpConfig::default();
        assert_eq!(config.bind_addr, "127.0.0.1:0".parse().unwrap());
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.buffer_size, 8192);
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
        assert_eq!(config.server_name.as_deref(), Some("EchoServer/1.0"));
        assert_eq!(config.default_content_type.as_deref(), Some("text/plain"));
        assert_eq!(config.max_body_size, DEFAULT_MAX_BODY_SIZE);
        assert!(config.bind_strategy.is_none());
        assert_eq!(config.service_name, "http");
        assert!(config.rate_limit.is_none());
        assert!(config.accept_rate_limit.is_none());
    }

    #[test]
    fn from_preserves_connection_fields() {
        let config = HttpConfig {
            bind_addr: "0.0.0.0:8000".parse().unwrap(),
            max_connections: 4,
            buffer_size: 55,
            read_timeout: Duration::from_millis(9),
            write_timeout: Duration::from_millis(10),
            server_name: None,
            default_content_type: None,
            max_body_size: 1,
            bind_strategy: Some(BindStrategy::Bind(BindTarget::Network(
                "127.0.0.1:8001".parse().unwrap(),
            ))),
            service_name: "api".into(),
            rate_limit: Some(RateLimitConfig::new(5, 6)),
            accept_rate_limit: Some(RateLimitConfig::new(7, 8)),
        };
        let stream: StreamConfig = config.into();
        assert_eq!(stream.bind_addr, "0.0.0.0:8000".parse().unwrap());
        assert_eq!(stream.max_connections, 4);
        assert_eq!(stream.buffer_size, 55);
        assert_eq!(stream.read_timeout, Duration::from_millis(9));
        assert_eq!(stream.write_timeout, Duration::from_millis(10));
        assert_eq!(stream.service_name, "api");
        assert_eq!(stream.rate_limit, Some(RateLimitConfig::new(5, 6)));
        assert_eq!(stream.accept_rate_limit, Some(RateLimitConfig::new(7, 8)));
        match stream.bind_strategy {
            Some(BindStrategy::Bind(BindTarget::Network(addr))) => {
                assert_eq!(addr, "127.0.0.1:8001".parse().unwrap())
            }
            other => panic!("strategy not preserved: {other:?}"),
        }
    }

    #[test]
    fn with_fd_inheritance_falls_back_to_current_bind_addr() {
        let mut config = HttpConfig::default().with_fd_inheritance("svc");
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
        let config = HttpConfig {
            bind_addr: "0.0.0.0:8080".parse().unwrap(),
            ..Default::default()
        }
        .with_fd_inheritance("web");
        assert_eq!(config.service_name, "web");
        // HTTP-specific fields are untouched.
        assert_eq!(config.max_body_size, DEFAULT_MAX_BODY_SIZE);
        match &config.bind_strategy {
            Some(BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: None,
            }) => {}
            other => panic!("unexpected strategy {other:?}"),
        }
    }
}
