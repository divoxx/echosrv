//! Configuration for the HTTP echo server.

use super::protocol::DEFAULT_MAX_BODY_SIZE;
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
/// };
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
    /// Network address to bind to.
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
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            max_connections: 100,
            buffer_size: 8192,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            server_name: Some("EchoServer/1.0".to_string()),
            default_content_type: Some("text/plain".to_string()),
            max_body_size: DEFAULT_MAX_BODY_SIZE,
        }
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
        }
    }
}
