use crate::stream::StreamConfig;
use std::time::Duration;

/// Configuration for HTTP echo server
///
/// Extends `StreamConfig` with HTTP-specific configuration options.
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
///     buffer_size: 8192, // Larger buffer for HTTP
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     server_name: Some("EchoServer/1.0".to_string()),
///     echo_headers: true,
///     default_content_type: Some("text/plain".to_string()),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Network address to bind to
    pub bind_addr: std::net::SocketAddr,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
    /// Server name to include in responses (optional)
    pub server_name: Option<String>,
    /// Whether to echo back request headers in response
    pub echo_headers: bool,
    /// Default content type for responses
    pub default_content_type: Option<String>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            max_connections: 100,
            buffer_size: 8192, // Larger buffer for HTTP requests
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            server_name: Some("EchoServer/1.0".to_string()),
            echo_headers: true,
            default_content_type: Some("text/plain".to_string()),
        }
    }
}

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