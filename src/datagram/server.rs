//! The generic datagram echo server, [`DatagramEchoServer`].

use super::{DatagramConfig, DatagramProtocol};
use crate::common::lifecycle::{
    ACCEPT_ERROR_BACKOFF, RateLimiters, ShutdownSignal, wait_for_shutdown,
};
use crate::common::{EchoServerTrait, ServerStats, payload_preview};
use crate::network::{Address, FdInheritanceConfig, LocalAddress};
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tracing::{debug, error, info, trace, warn};

/// Rate-limited drops are logged (at debug level) only once per this many,
/// so a flood does not flood the log as well.
const DROP_LOG_EVERY: u64 = 100;

/// Delay applied after a failed `recv_from` so a persistent error does not
/// turn the receive loop into a busy loop. Same as the stream accept backoff.
const RECV_ERROR_BACKOFF: Duration = ACCEPT_ERROR_BACKOFF;

/// While receives keep failing, only the first failure and then one in this
/// many are logged at error level; the others are logged at debug level.
/// With [`RECV_ERROR_BACKOFF`] this is about one error line every 10 seconds.
const RECV_ERROR_LOG_EVERY: u64 = 100;

/// Generic datagram-based echo server that works with any datagram protocol
///
/// This server can work with any protocol that implements `DatagramProtocol`,
/// such as UDP or Unix datagrams. Each received datagram is sent back to its
/// sender.
///
/// An optional [`DatagramConfig::rate_limit`] caps the datagram rate for the
/// whole server. Datagrams over the limit are dropped without a reply and
/// counted in [`stats`](Self::stats).
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::datagram::{DatagramConfig, DatagramEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use echosrv::udp::UdpProtocol;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = DatagramConfig {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///         ..Default::default()
///     };
///
///     let server: DatagramEchoServer<UdpProtocol> = DatagramEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Binding first to learn the actual address:
///
/// ```
/// use echosrv::{EchoClient, EchoServerTrait, UdpConfig, UdpEchoClient, UdpEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// let server = UdpEchoServer::new(UdpConfig::default().into()); // 127.0.0.1:0
/// let shutdown = server.shutdown_signal();
/// let bound = server.bind().await?;
/// let addr = *bound.local_addr().as_network().unwrap();
/// let handle = tokio::spawn(bound.serve());
///
/// let mut client = UdpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo_string("hi").await?, "hi");
///
/// shutdown.send(()).unwrap();
/// handle.await.unwrap()?;
/// # Ok(())
/// # }
/// ```
pub struct DatagramEchoServer<P: DatagramProtocol> {
    config: DatagramConfig,
    protocol: std::marker::PhantomData<P>,
    shutdown: ShutdownSignal,
    limiters: Arc<RateLimiters>,
    stats: Arc<ServerStats>,
}

impl<P: DatagramProtocol> DatagramEchoServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Creates a new datagram-based echo server with the given configuration
    pub fn new(config: DatagramConfig) -> Self {
        Self {
            limiters: Arc::new(RateLimiters::new(config.rate_limit, None)),
            stats: Arc::new(ServerStats::default()),
            config,
            protocol: std::marker::PhantomData,
            shutdown: ShutdownSignal::new(),
        }
    }

    /// The server configuration.
    pub fn config(&self) -> &DatagramConfig {
        &self.config
    }

    /// The server's counters (rate-limited drops).
    ///
    /// The handle is shared with every bound server created from this one,
    /// so it can be read while the server runs.
    pub fn stats(&self) -> Arc<ServerStats> {
        Arc::clone(&self.stats)
    }

    /// Validates the configuration and creates the socket.
    ///
    /// The returned [`BoundDatagramServer`] exposes the actual bound address
    /// and is `'static`, so it can be moved into `tokio::spawn`.
    pub async fn bind(&self) -> Result<BoundDatagramServer<P>> {
        self.config.validate()?;
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        let socket = P::bind_with_inheritance(&self.config, &fd_config)
            .await
            .map_err(Into::into)?;
        let local_addr = socket
            .local_address()
            .map_err(|e| P::map_io_error(e).into())?;
        Ok(BoundDatagramServer {
            socket,
            local_addr,
            config: self.config.clone(),
            shutdown_rx: self.shutdown.receiver(),
            limiters: Arc::clone(&self.limiters),
            stats: Arc::clone(&self.stats),
        })
    }
}

/// A datagram echo server whose socket has been created.
///
/// Obtained from [`DatagramEchoServer::bind`].
pub struct BoundDatagramServer<P: DatagramProtocol> {
    socket: P::Socket,
    local_addr: Address,
    config: DatagramConfig,
    shutdown_rx: broadcast::Receiver<()>,
    limiters: Arc<RateLimiters>,
    stats: Arc<ServerStats>,
}

impl<P: DatagramProtocol> BoundDatagramServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// The address the socket is bound to (resolves port `0`).
    pub fn local_addr(&self) -> &Address {
        &self.local_addr
    }

    /// The server's counters; the same handle as [`DatagramEchoServer::stats`].
    pub fn stats(&self) -> Arc<ServerStats> {
        Arc::clone(&self.stats)
    }

    /// Echoes datagrams until shutdown is requested, then returns `Ok(())`.
    pub async fn serve(self) -> Result<()> {
        let Self {
            socket,
            local_addr,
            config,
            mut shutdown_rx,
            limiters,
            stats,
        } = self;

        info!(address = %local_addr, "Datagram echo server listening");

        let mut buffer = vec![0; config.buffer_size];
        // Consecutive `recv_from` failures, reset by a successful receive.
        let mut recv_errors: u64 = 0;

        loop {
            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    info!("Shutdown requested, stopping datagram echo server");
                    break;
                }
                received = timeout(config.read_timeout, P::recv_from(&socket, &mut buffer)) => {
                    match received {
                        Ok(Ok((n, peer))) => {
                            if recv_errors > 0 {
                                info!(failures = recv_errors, "Receiving datagrams again after errors");
                                recv_errors = 0;
                            }
                            if limiters.check_request().is_err() {
                                // There is no way to signal a rejection on a
                                // datagram socket: drop it.
                                let total = stats.inc_dropped_rate_limited();
                                if total % DROP_LOG_EVERY == 1 {
                                    debug!(?peer, total, "Datagram dropped: rate limited (logged every {DROP_LOG_EVERY})");
                                }
                                continue;
                            }
                            trace!(?peer, size = n, preview = %payload_preview(&buffer[..n]), "Received datagram");
                            match timeout(config.write_timeout, P::send_to(&socket, &buffer[..n], &peer)).await {
                                Ok(Ok(_)) => trace!(?peer, size = n, "Echoed datagram"),
                                Ok(Err(e)) => warn!(?peer, error = %e, "Failed to send echo response"),
                                Err(_) => warn!(?peer, "Timed out sending echo response"),
                            }
                        }
                        Ok(Err(e)) => {
                            recv_errors += 1;
                            if recv_errors == 1 || recv_errors % RECV_ERROR_LOG_EVERY == 0 {
                                error!(error = %e, failures = recv_errors, "Failed to receive datagram (logged every {RECV_ERROR_LOG_EVERY} while failing)");
                            } else {
                                debug!(error = %e, failures = recv_errors, "Failed to receive datagram");
                            }
                            // Back off, but stay responsive to shutdown.
                            tokio::select! {
                                biased;
                                _ = wait_for_shutdown(&mut shutdown_rx) => {
                                    info!("Shutdown requested, stopping datagram echo server");
                                    break;
                                }
                                () = tokio::time::sleep(RECV_ERROR_BACKOFF) => {}
                            }
                        }
                        Err(_) => {
                            trace!("Receive timeout (idle)");
                        }
                    }
                }
            }
        }

        info!("Datagram echo server stopped");
        Ok(())
    }
}

#[async_trait]
impl<P: DatagramProtocol + Sync> EchoServerTrait for DatagramEchoServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Binds (see [`DatagramEchoServer::bind`]) and serves until shutdown.
    async fn run(&self) -> Result<()> {
        self.bind().await?.serve().await
    }

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    fn shutdown_signal(&self) -> broadcast::Sender<()> {
        self.shutdown.sender()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    /// Calls after which the failing socket stops returning errors and blocks,
    /// so a missing backoff shows up as a high count rather than a hang.
    const MAX_FAILURES: usize = 50;

    /// A socket whose every receive fails.
    #[derive(Default)]
    struct FailingSocket {
        calls: AtomicUsize,
        called: Notify,
    }

    impl LocalAddress for Arc<FailingSocket> {
        fn local_address(&self) -> std::io::Result<Address> {
            Ok(Address::Network("127.0.0.1:0".parse().unwrap()))
        }
    }

    struct FailingProtocol;

    #[async_trait]
    impl DatagramProtocol for FailingProtocol {
        type Error = std::io::Error;
        type Socket = Arc<FailingSocket>;
        type PeerAddr = ();

        async fn bind(_config: &DatagramConfig) -> std::io::Result<Self::Socket> {
            Ok(Arc::default())
        }

        async fn recv_from(
            socket: &Self::Socket,
            _buffer: &mut [u8],
        ) -> std::io::Result<(usize, ())> {
            if socket.calls.fetch_add(1, Ordering::SeqCst) >= MAX_FAILURES {
                std::future::pending::<()>().await;
            }
            socket.called.notify_one();
            Err(std::io::Error::other("socket in a bad state"))
        }

        async fn send_to(
            _socket: &Self::Socket,
            data: &[u8],
            _addr: &(),
        ) -> std::io::Result<usize> {
            Ok(data.len())
        }

        fn map_io_error(err: std::io::Error) -> Self::Error {
            err
        }
    }

    /// A bound server over a failing socket, the socket to observe it, and
    /// its shutdown sender.
    fn failing_server() -> (
        BoundDatagramServer<FailingProtocol>,
        Arc<FailingSocket>,
        broadcast::Sender<()>,
    ) {
        let socket = Arc::new(FailingSocket::default());
        let shutdown = ShutdownSignal::new();
        let server = BoundDatagramServer {
            socket: Arc::clone(&socket),
            local_addr: socket.local_address().unwrap(),
            config: DatagramConfig::default(),
            shutdown_rx: shutdown.receiver(),
            limiters: Arc::new(RateLimiters::new(None, None)),
            stats: Arc::new(ServerStats::default()),
        };
        (server, socket, shutdown.sender())
    }

    #[tokio::test(start_paused = true)]
    async fn recv_errors_back_off() {
        let (server, socket, shutdown) = failing_server();
        let handle = tokio::spawn(server.serve());

        // One receive right away, then one per backoff period.
        tokio::time::sleep(RECV_ERROR_BACKOFF * 10 + RECV_ERROR_BACKOFF / 2).await;
        assert_eq!(socket.calls.load(Ordering::SeqCst), 11);

        shutdown.send(()).unwrap();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_interrupts_backoff() {
        let (server, socket, shutdown) = failing_server();
        let handle = tokio::spawn(server.serve());

        // The server is now backing off after its first failed receive.
        socket.called.notified().await;
        shutdown.send(()).unwrap();

        // Paused time only advances when every task is idle, so a server
        // still sleeping in the backoff would let this timeout fire first.
        timeout(RECV_ERROR_BACKOFF / 10, handle)
            .await
            .expect("shutdown waited for the backoff")
            .unwrap()
            .unwrap();
        assert_eq!(socket.calls.load(Ordering::SeqCst), 1);
    }
}
