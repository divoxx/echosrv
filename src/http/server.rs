//! The HTTP echo server.

use super::config::HttpConfig;
use super::protocol::{HTTP_SETTINGS, HttpProtocol, HttpSettings};
use crate::Result;
use crate::common::EchoServerTrait;
use crate::network::Address;
use crate::stream::{BoundStreamServer, StreamConfig, StreamEchoServer};
use async_trait::async_trait;
use std::sync::Arc;

/// HTTP/1.1 echo server: echoes the body of each `POST` request.
///
/// This is a thin wrapper around `StreamEchoServer<HttpProtocol>`. It reuses
/// the generic accept loop, connection limit, timeouts, socket inheritance
/// and shutdown handling, and also applies the HTTP-specific [`HttpConfig`]
/// fields to every accepted connection. See the [`crate::http`] module docs
/// for the exact HTTP semantics.
///
/// # Examples
///
/// ```no_run
/// use echosrv::http::{HttpConfig, HttpEchoServer};
/// use echosrv::EchoServerTrait;
///
/// #[tokio::main]
/// async fn main() -> echosrv::Result<()> {
///     let server = HttpEchoServer::new(HttpConfig {
///         bind_addr: "127.0.0.1:8080".parse().unwrap(),
///         ..HttpConfig::default()
///     });
///     server.run().await
/// }
/// ```
///
/// Binding first to learn the actual address (e.g. when using port 0):
///
/// ```
/// use echosrv::http::{HttpConfig, HttpEchoClient, HttpEchoServer};
/// use echosrv::{EchoClient, EchoServerTrait};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// let server = HttpEchoServer::new(HttpConfig::default()); // 127.0.0.1:0
/// let shutdown = server.shutdown_signal();
/// let bound = server.bind().await?;
/// let addr = *bound.local_addr().as_network().unwrap();
/// let handle = tokio::spawn(bound.serve());
///
/// let mut client = HttpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo(b"hi").await?, b"hi");
///
/// shutdown.send(()).unwrap();
/// handle.await.unwrap()?;
/// # Ok(())
/// # }
/// ```
pub struct HttpEchoServer {
    inner: StreamEchoServer<HttpProtocol>,
    settings: Arc<HttpSettings>,
}

impl HttpEchoServer {
    /// Creates a new HTTP echo server from `config`.
    pub fn new(config: HttpConfig) -> Self {
        let settings = Arc::new(HttpSettings::from(&config));
        Self {
            inner: StreamEchoServer::new(config.into()),
            settings,
        }
    }

    /// The connection-level configuration (the HTTP-specific fields of
    /// [`HttpConfig`] are not part of it).
    pub fn config(&self) -> &StreamConfig {
        self.inner.config()
    }

    /// Validates the configuration and creates the listening socket.
    ///
    /// See [`StreamEchoServer::bind`]. The returned [`BoundHttpServer`] is
    /// `'static`, exposes the bound address and is served with
    /// [`BoundHttpServer::serve`]. Shutdown is still requested through this
    /// server's [`shutdown_signal`](EchoServerTrait::shutdown_signal).
    pub async fn bind(&self) -> Result<BoundHttpServer> {
        Ok(BoundHttpServer {
            inner: self.inner.bind().await?,
            settings: Arc::clone(&self.settings),
        })
    }
}

/// An HTTP echo server whose listening socket has been created.
///
/// Obtained from [`HttpEchoServer::bind`].
pub struct BoundHttpServer {
    inner: BoundStreamServer<HttpProtocol>,
    settings: Arc<HttpSettings>,
}

impl BoundHttpServer {
    /// The address the listener is bound to (resolves port `0`).
    pub fn local_addr(&self) -> &Address {
        self.inner.local_addr()
    }

    /// Accepts and serves HTTP connections until shutdown is requested.
    ///
    /// See [`BoundStreamServer::serve`].
    pub async fn serve(self) -> Result<()> {
        // `BoundStreamServer::serve` calls `HttpProtocol::accept` directly in
        // its own future (only the per-connection handlers are spawned), so
        // accept sees this task-local scope and hands the settings to every
        // `HttpStream` it creates.
        HTTP_SETTINGS.scope(self.settings, self.inner.serve()).await
    }
}

#[async_trait]
impl EchoServerTrait for HttpEchoServer {
    /// Binds (see [`HttpEchoServer::bind`]) and serves until shutdown.
    async fn run(&self) -> Result<()> {
        self.bind().await?.serve().await
    }

    /// Returns a sender that gracefully stops [`run`](Self::run) when
    /// signalled.
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.inner.shutdown_signal()
    }
}
