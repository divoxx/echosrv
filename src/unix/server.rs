use crate::Result;
use crate::common::EchoServerTrait;
use crate::datagram::{BoundDatagramServer, DatagramEchoServer};
use crate::stream::{BoundStreamServer, StreamEchoServer};
use crate::unix::config::{UnixDatagramConfig, UnixStreamConfig};
use crate::unix::datagram_protocol::UnixDatagramProtocol;
use crate::unix::stream_protocol::UnixStreamProtocol;
use async_trait::async_trait;

/// Unix domain stream echo server
///
/// A thin wrapper around [`StreamEchoServer`]`<`[`UnixStreamProtocol`]`>` that
/// accepts a [`UnixStreamConfig`]. It shares the generic server's behavior:
/// `max_connections` enforcement, timeouts and graceful shutdown.
///
/// Socket file handling:
/// * at bind, a stale socket file (nothing listening) is removed and re-bound;
///   a live socket or non-socket file is an error,
/// * on shutdown, the socket file is removed only if this server created it
///   (never for inherited sockets).
///
/// # Examples
///
/// ```no_run
/// use echosrv::unix::{UnixStreamConfig, UnixStreamEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = UnixStreamConfig {
///         max_connections: 100,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///         ..UnixStreamConfig::default().with_socket_path("/tmp/echo.sock".into())
///     };
///
///     let server = UnixStreamEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
pub struct UnixStreamEchoServer {
    inner: StreamEchoServer<UnixStreamProtocol>,
}

impl UnixStreamEchoServer {
    /// Creates a new Unix domain stream echo server with the given configuration
    pub fn new(config: UnixStreamConfig) -> Self {
        Self {
            inner: StreamEchoServer::new(config.into()),
        }
    }

    /// Creates the listening socket; see [`StreamEchoServer::bind`].
    pub async fn bind(&self) -> Result<BoundStreamServer<UnixStreamProtocol>> {
        self.inner.bind().await
    }
}

#[async_trait]
impl EchoServerTrait for UnixStreamEchoServer {
    async fn run(&self) -> Result<()> {
        self.inner.run().await
    }

    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.inner.shutdown_signal()
    }
}

/// Unix domain datagram echo server
///
/// A thin wrapper around [`DatagramEchoServer`]`<`[`UnixDatagramProtocol`]`>`
/// that accepts a [`UnixDatagramConfig`]. Each datagram is echoed to the
/// sender's socket path (senders must be bound to a path to get replies).
/// Socket file handling matches [`UnixStreamEchoServer`].
///
/// # Examples
///
/// ```no_run
/// use echosrv::unix::{UnixDatagramConfig, UnixDatagramEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = UnixDatagramConfig {
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///         ..UnixDatagramConfig::default().with_socket_path("/tmp/echo_dgram.sock".into())
///     };
///
///     let server = UnixDatagramEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
pub struct UnixDatagramEchoServer {
    inner: DatagramEchoServer<UnixDatagramProtocol>,
}

impl UnixDatagramEchoServer {
    /// Creates a new Unix domain datagram echo server with the given configuration
    pub fn new(config: UnixDatagramConfig) -> Self {
        Self {
            inner: DatagramEchoServer::new(config.into()),
        }
    }

    /// Creates the socket; see [`DatagramEchoServer::bind`].
    pub async fn bind(&self) -> Result<BoundDatagramServer<UnixDatagramProtocol>> {
        self.inner.bind().await
    }
}

#[async_trait]
impl EchoServerTrait for UnixDatagramEchoServer {
    async fn run(&self) -> Result<()> {
        self.inner.run().await
    }

    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.inner.shutdown_signal()
    }
}
