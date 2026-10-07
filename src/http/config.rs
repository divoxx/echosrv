//! Configuration for the HTTP echo server.

use super::protocol::DEFAULT_MAX_BODY_SIZE;
use crate::network::BindStrategy;
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
        }
    }
}

impl HttpConfig {
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
        };
        let stream: StreamConfig = config.into();
        assert_eq!(stream.bind_addr, "0.0.0.0:8000".parse().unwrap());
        assert_eq!(stream.max_connections, 4);
        assert_eq!(stream.buffer_size, 55);
        assert_eq!(stream.read_timeout, Duration::from_millis(9));
        assert_eq!(stream.write_timeout, Duration::from_millis(10));
        assert_eq!(stream.service_name, "api");
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
