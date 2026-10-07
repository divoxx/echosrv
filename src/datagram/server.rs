//! The generic datagram echo server, [`DatagramEchoServer`].

use super::{DatagramConfig, DatagramProtocol};
use crate::common::EchoServerTrait;
use crate::common::lifecycle::{ShutdownSignal, wait_for_shutdown};
use crate::network::{Address, FdInheritanceConfig, LocalAddress};
use crate::{EchoError, Result};
use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tracing::{error, info, trace, warn};

/// Generic datagram-based echo server that works with any datagram protocol
///
/// This server can work with any protocol that implements `DatagramProtocol`,
/// such as UDP or Unix datagrams. Each received datagram is sent back to its
/// sender.
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
}

impl<P: DatagramProtocol> DatagramEchoServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Creates a new datagram-based echo server with the given configuration
    pub fn new(config: DatagramConfig) -> Self {
        Self {
            config,
            protocol: std::marker::PhantomData,
            shutdown: ShutdownSignal::new(),
        }
    }

    /// The server configuration.
    pub fn config(&self) -> &DatagramConfig {
        &self.config
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
}

impl<P: DatagramProtocol> BoundDatagramServer<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// The address the socket is bound to (resolves port `0`).
    pub fn local_addr(&self) -> &Address {
        &self.local_addr
    }

    /// Echoes datagrams until shutdown is requested, then returns `Ok(())`.
    pub async fn serve(self) -> Result<()> {
        let Self {
            socket,
            local_addr,
            config,
            mut shutdown_rx,
        } = self;

        info!(address = %local_addr, "Datagram echo server listening");

        let mut buffer = vec![0; config.buffer_size];

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
                            trace!(?peer, size = n, preview = %String::from_utf8_lossy(&buffer[..n]), "Received datagram");
                            match timeout(config.write_timeout, P::send_to(&socket, &buffer[..n], &peer)).await {
                                Ok(Ok(_)) => trace!(?peer, size = n, "Echoed datagram"),
                                Ok(Err(e)) => warn!(?peer, error = %e, "Failed to send echo response"),
                                Err(_) => warn!(?peer, "Timed out sending echo response"),
                            }
                        }
                        Ok(Err(e)) => {
                            error!(error = %e, "Failed to receive datagram");
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
