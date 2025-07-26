use crate::{Result, EchoError};
use crate::common::EchoServer;
use super::config::UdpConfig;

use std::sync::Arc;
use tokio::{
    net::UdpSocket,
    signal,
    time::timeout,
};
use tracing::{error, info, warn};

/// UDP echo server that handles UDP datagrams
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::udp::{UdpConfig, UdpEchoServer};
/// use echosrv::common::EchoServer;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = UdpConfig {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///     };
///
///     let server = UdpEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Server with graceful shutdown:
///
/// ```no_run
/// use echosrv::udp::{UdpConfig, UdpEchoServer};
/// use echosrv::common::EchoServer;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = UdpConfig::default();
///     let server = UdpEchoServer::new(config);
///     let shutdown_signal = server.shutdown_signal();
///
///     // Run server in background
///     let server_handle = tokio::spawn(async move {
///         server.run().await
///     });
///
///     // Do other work...
///     
///     // Gracefully shutdown
///     let _ = shutdown_signal.send(());
///     server_handle.await??;
///     Ok(())
/// }
/// ```
pub struct UdpEchoServer {
    config: UdpConfig,
    shutdown_signal: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl UdpEchoServer {
    /// Creates a new UDP echo server with the given configuration
    pub fn new(config: UdpConfig) -> Self {
        let (shutdown_signal, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            shutdown_signal: Arc::new(shutdown_signal),
        }
    }
}

impl EchoServer for UdpEchoServer {
    /// Starts the UDP echo server and listens for datagrams
    async fn run(&self) -> Result<()> {
        let socket = UdpSocket::bind(self.config.bind_addr)
            .await
            .map_err(|e| EchoError::Config(format!("Failed to bind to {}: {}", self.config.bind_addr, e)))?;

        info!(address = %self.config.bind_addr, "UDP echo server listening");

        let mut buffer = vec![0; self.config.buffer_size];
        let mut shutdown_rx = self.shutdown_signal.subscribe();

        loop {
            tokio::select! {
                res = timeout(self.config.read_timeout, socket.recv_from(&mut buffer)) => {
                    match res {
                        Ok(Ok((n, addr))) => {
                            let preview = String::from_utf8_lossy(&buffer[..n]);
                            info!(%addr, size = n, preview = %preview, "Received datagram");
                            if let Err(e) = socket.send_to(&buffer[..n], addr).await {
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

        info!("UDP echo server stopped");
        Ok(())
    }

    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_signal.as_ref().clone()
    }
} 