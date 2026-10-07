//! The HTTP echo server.

use super::config::HttpConfig;
use super::protocol::{HTTP_SETTINGS, HttpProtocol, HttpSettings};
use crate::Result;
use crate::common::EchoServerTrait;
use crate::stream::StreamEchoServer;
use async_trait::async_trait;
use std::sync::Arc;

/// HTTP/1.1 echo server: echoes the body of each `POST` request.
///
/// This is a thin wrapper around `StreamEchoServer<HttpProtocol>`. It reuses
/// the generic accept loop, connection limit and timeouts, and also applies
/// the HTTP-specific [`HttpConfig`] fields to every accepted connection. See
/// the [`crate::http`] module docs for the exact HTTP semantics.
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
}

#[async_trait]
impl EchoServerTrait for HttpEchoServer {
    /// Binds, then serves HTTP requests until shut down.
    async fn run(&self) -> Result<()> {
        // `HttpProtocol::accept` runs inside this future, so it can read the
        // settings from the task-local scope.
        HTTP_SETTINGS
            .scope(Arc::clone(&self.settings), self.inner.run())
            .await
    }

    /// Returns a sender that gracefully stops [`run`](Self::run) when
    /// signalled.
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.inner.shutdown_signal()
    }
}
