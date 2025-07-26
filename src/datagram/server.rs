use crate::{Result, EchoError};
use crate::common::EchoServerTrait;
use super::{DatagramConfig, DatagramProtocol};
use std::sync::Arc;
use tokio::{
    signal,
    time::timeout,
};
use tracing::{error, info, warn};
use async_trait::async_trait;

/// Generic datagram-based echo server that works with any datagram protocol
///
/// This server can work with any protocol that implements `DatagramProtocol`,
/// such as UDP, Unix datagrams, etc.
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
///     };
///
///     let server: DatagramEchoServer<UdpProtocol> = DatagramEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
pub struct DatagramEchoServer<P: DatagramProtocol> {
    config: DatagramConfig,
    protocol: std::marker::PhantomData<P>,
    shutdown_signal: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl<P: DatagramProtocol> DatagramEchoServer<P> 
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Creates a new datagram-based echo server with the given configuration
    pub fn new(config: DatagramConfig) -> Self {
        let (shutdown_signal, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            protocol: std::marker::PhantomData,
            shutdown_signal: Arc::new(shutdown_signal),
        }
    }
}

#[async_trait]
impl<P: DatagramProtocol + Sync> EchoServerTrait for DatagramEchoServer<P> 
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Starts the datagram-based echo server and listens for datagrams
    async fn run(&self) -> Result<()> {
        let socket = P::bind(&self.config).await.map_err(|e| e.into())?;

        info!(address = %self.config.bind_addr, "Datagram echo server listening");

        let mut buffer = vec![0; self.config.buffer_size];
        let mut shutdown_rx = self.shutdown_signal.subscribe();

        loop {
            tokio::select! {
                recv_result = timeout(self.config.read_timeout, P::recv_from(&socket, &mut buffer)) => {
                    match recv_result {
                        Ok(Ok((n, addr))) => {
                            let preview = String::from_utf8_lossy(&buffer[..n]);
                            info!(%addr, size = n, preview = %preview, "Received datagram");
                            
                            if let Err(e) = P::send_to(&socket, &buffer[..n], addr).await {
                                error!(%addr, error = %e, "Failed to send echo response");
                            } else {
                                info!(%addr, size = n, "Echoed datagram");
                            }
                        }
                        Ok(Err(e)) => {
                            error!(error = %e, "Failed to receive datagram");
                        }
                        Err(_) => {
                            warn!("Receive timeout");
                        }
                    }
                }
                _ = signal::ctrl_c() => {
                    info!("Received shutdown signal, stopping server");
                    break;
                }
                _ = shutdown_rx.recv() => {
                    info!("Received internal shutdown signal, stopping server");
                    break;
                }
            }
        }

        info!("Datagram echo server stopped");
        Ok(())
    }

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_signal.as_ref().clone()
    }
} 