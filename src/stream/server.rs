//! The generic stream echo server, [`StreamEchoServer`].

use super::{RejectReason, StreamConfig, StreamProtocol};
use crate::common::lifecycle::{
    ACCEPT_ERROR_BACKOFF, ConnectionGuard, MAX_PENDING_REJECTIONS, REJECT_TIMEOUT, RateLimiters,
    ShutdownSignal, wait_for_shutdown,
};
use crate::common::{EchoServerTrait, ServerStats};
use crate::network::{Address, FdInheritanceConfig, LocalAddress};
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, trace, warn};

/// Generic stream-based echo server that works with any stream protocol
///
/// This server can work with any protocol that implements `StreamProtocol`,
/// such as TCP, HTTP or Unix streams.
///
/// Features:
/// * bounded concurrency (`max_connections`; extra connections are rejected
///   through [`StreamProtocol::reject`] with
///   [`RejectReason::TooManyConnections`] and counted in
///   [`stats`](Self::stats)),
/// * per-read and per-write timeouts,
/// * optional request and new-connection rate limits
///   ([`StreamConfig::rate_limit`], [`StreamConfig::accept_rate_limit`]);
///   over-limit traffic is rejected through [`StreamProtocol::reject`] and
///   counted in [`stats`](Self::stats),
/// * graceful shutdown via [`EchoServerTrait::shutdown_signal`] — in-flight
///   connections are cancelled and awaited before `run()` returns,
/// * socket inheritance via [`StreamConfig::bind_strategy`].
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::stream::{StreamConfig, StreamEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use echosrv::tcp::TcpProtocol;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = StreamConfig {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         max_connections: 100,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///         ..Default::default()
///     };
///
///     let server: StreamEchoServer<TcpProtocol> = StreamEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Binding first to learn the actual address (e.g. when using port 0):
///
/// ```
/// use echosrv::{EchoClient, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// let server = TcpEchoServer::new(TcpConfig::default().into()); // 127.0.0.1:0
/// let shutdown = server.shutdown_signal();
/// let bound = server.bind().await?;
/// let addr = *bound.local_addr().as_network().unwrap();
/// let handle = tokio::spawn(bound.serve());
///
/// let mut client = TcpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo_string("hi").await?, "hi");
///
/// shutdown.send(()).unwrap();
/// handle.await.unwrap()?;
/// # Ok(())
/// # }
/// ```
pub struct StreamEchoServer<P: StreamProtocol> {
    config: StreamConfig,
    protocol: std::marker::PhantomData<P>,
    shutdown: ShutdownSignal,
    limiters: Arc<RateLimiters>,
    stats: Arc<ServerStats>,
}

impl<P: StreamProtocol> StreamEchoServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Creates a new stream-based echo server with the given configuration
    pub fn new(config: StreamConfig) -> Self {
        Self {
            limiters: Arc::new(RateLimiters::new(
                config.rate_limit,
                config.accept_rate_limit,
            )),
            stats: Arc::new(ServerStats::default()),
            config,
            protocol: std::marker::PhantomData,
            shutdown: ShutdownSignal::new(),
        }
    }

    /// The server configuration.
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// The server's counters (rate-limit and connection-limit rejections).
    ///
    /// The handle is shared with every bound server created from this one,
    /// so it can be read while the server runs.
    pub fn stats(&self) -> Arc<ServerStats> {
        Arc::clone(&self.stats)
    }

    /// Validates the configuration and creates the listening socket.
    ///
    /// The returned [`BoundStreamServer`] exposes the actual bound address and
    /// is served with [`BoundStreamServer::serve`]. It is `'static`, so it can
    /// be moved into `tokio::spawn`. Shutdown is still requested through this
    /// server's [`shutdown_signal`](EchoServerTrait::shutdown_signal).
    pub async fn bind(&self) -> Result<BoundStreamServer<P>> {
        self.config.validate()?;
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        let listener = P::bind_with_inheritance(&self.config, &fd_config)
            .await
            .map_err(Into::into)?;
        let local_addr = listener
            .local_address()
            .map_err(|e| P::map_io_error(e).into())?;
        Ok(BoundStreamServer {
            listener,
            local_addr,
            config: self.config.clone(),
            shutdown_rx: self.shutdown.receiver(),
            limiters: Arc::clone(&self.limiters),
            stats: Arc::clone(&self.stats),
        })
    }
}

/// A stream echo server whose listening socket has been created.
///
/// Obtained from [`StreamEchoServer::bind`].
pub struct BoundStreamServer<P: StreamProtocol> {
    listener: P::Listener,
    local_addr: Address,
    config: StreamConfig,
    shutdown_rx: broadcast::Receiver<()>,
    limiters: Arc<RateLimiters>,
    stats: Arc<ServerStats>,
}

impl<P> BoundStreamServer<P>
where
    P: StreamProtocol + 'static,
    P::Error: Into<EchoError> + std::fmt::Display,
    P::Stream: 'static,
{
    /// The address the listener is bound to (resolves port `0`).
    pub fn local_addr(&self) -> &Address {
        &self.local_addr
    }

    /// The server's counters; the same handle as [`StreamEchoServer::stats`].
    pub fn stats(&self) -> Arc<ServerStats> {
        Arc::clone(&self.stats)
    }

    /// Accepts and serves connections until shutdown is requested.
    ///
    /// On shutdown the listener is closed, in-flight connection tasks are
    /// cancelled and awaited, and `Ok(())` is returned.
    pub async fn serve(self) -> Result<()> {
        let Self {
            mut listener,
            local_addr,
            config,
            mut shutdown_rx,
            limiters,
            stats,
        } = self;

        info!(address = %local_addr, max_connections = config.max_connections, "Stream echo server listening");

        let config = Arc::new(config);
        let limits = Limits { limiters, stats };
        let active = Arc::new(AtomicUsize::new(0));
        let rejecting = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        loop {
            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    info!("Shutdown requested, stopping stream echo server");
                    break;
                }
                Some(joined) = tasks.join_next(), if !tasks.is_empty() => {
                    if let Err(e) = joined {
                        if e.is_panic() {
                            error!(error = %e, "Connection task panicked");
                        }
                    }
                }
                accepted = P::accept(&mut listener) => {
                    match accepted {
                        Ok((stream, addr)) => {
                            let span = tracing::debug_span!("connection", %addr);
                            let admitted = match ConnectionGuard::try_acquire(&active, config.max_connections) {
                                None => {
                                    let total = limits.stats.inc_rejected_over_capacity();
                                    debug!(%addr, limit = config.max_connections, total, "Connection rejected: limit reached");
                                    Err((RejectReason::TooManyConnections, Duration::ZERO))
                                }
                                Some(guard) => match limits.limiters.check_connection() {
                                    Ok(()) => Ok(guard),
                                    Err(limited) => {
                                        let total = limits.stats.inc_rejected_connections();
                                        debug!(%addr, retry_after = ?limited.retry_after, total, "Connection rejected: accept rate limited");
                                        Err((RejectReason::ConnectionRateLimited, limited.retry_after))
                                    }
                                },
                            };
                            let guard = match admitted {
                                Ok(guard) => guard,
                                Err((reason, retry_after)) => {
                                    // Rejecting may involve I/O (e.g. HTTP reads the
                                    // request and answers 429 or 503), so do it off the
                                    // accept loop, in a pool of its own: rejections must
                                    // not take the slots of admitted connections.
                                    let Some(slot) = ConnectionGuard::try_acquire(&rejecting, MAX_PENDING_REJECTIONS) else {
                                        debug!(%addr, ?reason, "Too many pending rejections, closing connection");
                                        drop(stream);
                                        continue;
                                    };
                                    let cancel = cancel.clone();
                                    tasks.spawn(
                                        async move {
                                            let _slot = slot;
                                            let mut stream = stream;
                                            tokio::select! {
                                                () = send_rejection::<P>(&mut stream, reason, retry_after) => {}
                                                _ = cancel.cancelled() => {}
                                            }
                                        }
                                        .instrument(span),
                                    );
                                    continue;
                                }
                            };
                            let cancel = cancel.clone();

                            debug!(%addr, active = guard.active(), "Accepted connection");

                            let config = Arc::clone(&config);
                            let limits = limits.clone();
                            tasks.spawn(
                                async move {
                                    let _guard = guard;
                                    tokio::select! {
                                        result = handle_connection::<P>(stream, addr, &config, &limits) => {
                                            if let Err(e) = result {
                                                warn!(error = %e, "Error handling connection");
                                            }
                                        }
                                        _ = cancel.cancelled() => {
                                            debug!("Connection closed by server shutdown");
                                        }
                                    }
                                    debug!("Connection closed");
                                }
                                .instrument(span),
                            );
                        }
                        Err(e) => {
                            let e: EchoError = e.into();
                            error!(error = %e, "Failed to accept connection");
                            tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                        }
                    }
                }
            }
        }

        // Stop accepting (closes the socket, removes owned Unix socket files).
        drop(listener);
        cancel.cancel();
        while tasks.join_next().await.is_some() {}

        info!("Stream echo server stopped");
        Ok(())
    }
}

/// Rate limiters and counters shared by the connection tasks of one server.
#[derive(Clone)]
struct Limits {
    limiters: Arc<RateLimiters>,
    stats: Arc<ServerStats>,
}

impl Limits {
    /// Admits one request, or rejects it: counts and logs the rejection and
    /// lets the protocol tell the peer. Returns `false` if rejected, in which
    /// case the caller must close the connection.
    async fn admit_request<P>(&self, stream: &mut P::Stream, addr: SocketAddr) -> bool
    where
        P: StreamProtocol,
        P::Error: Into<EchoError> + std::fmt::Display,
    {
        let Err(limited) = self.limiters.check_request() else {
            return true;
        };
        let total = self.stats.inc_rejected_requests();
        debug!(%addr, retry_after = ?limited.retry_after, total, "Request rejected: rate limited");
        send_rejection::<P>(stream, RejectReason::RateLimited, limited.retry_after).await;
        false
    }
}

/// Runs [`StreamProtocol::reject`] bounded by [`REJECT_TIMEOUT`]. Failures are
/// only logged (at debug level): the connection is closed either way.
async fn send_rejection<P>(stream: &mut P::Stream, reason: RejectReason, retry_after: Duration)
where
    P: StreamProtocol,
    P::Error: Into<EchoError> + std::fmt::Display,
{
    match timeout(REJECT_TIMEOUT, P::reject(stream, reason, retry_after)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => debug!(error = %e, ?reason, "Failed to signal rejection"),
        Err(_) => debug!(?reason, "Timed out signalling rejection"),
    }
}

/// Echoes everything read from `stream` back to it until EOF, error, timeout
/// or a rate-limit rejection.
async fn handle_connection<P>(
    mut stream: P::Stream,
    addr: SocketAddr,
    config: &StreamConfig,
    limits: &Limits,
) -> Result<()>
where
    P: StreamProtocol,
    P::Error: Into<EchoError> + std::fmt::Display,
{
    if P::FRAMED_REQUESTS {
        match timeout(config.read_timeout, P::begin_request(&mut stream)).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return Ok(()),
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                debug!(%addr, "Read timeout, closing idle connection");
                return Ok(());
            }
        }
        if !limits.admit_request::<P>(&mut stream, addr).await {
            return Ok(());
        }
    }

    let mut buffer = vec![0; config.buffer_size];

    loop {
        let n = match timeout(config.read_timeout, P::read(&mut stream, &mut buffer)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                debug!(%addr, "Read timeout, closing idle connection");
                break;
            }
        };

        if n == 0 {
            debug!(%addr, "Client closed connection");
            break;
        }

        if !P::FRAMED_REQUESTS && !limits.admit_request::<P>(&mut stream, addr).await {
            break;
        }

        trace!(%addr, size = n, preview = %String::from_utf8_lossy(&buffer[..n]), "Received data");

        let write = async {
            P::write(&mut stream, &buffer[..n]).await?;
            P::flush(&mut stream).await
        };
        match timeout(config.write_timeout, write).await {
            Ok(Ok(())) => trace!(%addr, size = n, "Echoed data"),
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                warn!(%addr, "Write timeout");
                break;
            }
        }
    }

    Ok(())
}

#[async_trait]
impl<P: StreamProtocol + Sync + 'static> EchoServerTrait for StreamEchoServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
    P::Stream: 'static,
{
    /// Binds (see [`StreamEchoServer::bind`]) and serves until shutdown.
    async fn run(&self) -> Result<()> {
        self.bind().await?.serve().await
    }

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    fn shutdown_signal(&self) -> broadcast::Sender<()> {
        self.shutdown.sender()
    }
}
