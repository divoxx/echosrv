use crate::{Result, EchoError};
use crate::common::EchoServer;
use super::config::TcpConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    signal,
    time::timeout,
};
use tracing::{error, info, warn, Instrument};

/// TCP echo server that handles TCP connections
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::tcp::{TcpConfig, TcpEchoServer};
/// use echosrv::common::EchoServer;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = TcpConfig {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         max_connections: 100,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///     };
///
///     let server = TcpEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Server with graceful shutdown:
///
/// ```no_run
/// use echosrv::tcp::{TcpConfig, TcpEchoServer};
/// use echosrv::common::EchoServer;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = TcpConfig::default();
///     let server = TcpEchoServer::new(config);
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
pub struct TcpEchoServer {
    config: TcpConfig,
    shutdown_signal: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl TcpEchoServer {
    /// Creates a new TCP echo server with the given configuration
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::tcp::{TcpConfig, TcpEchoServer};
    ///
    /// let config = TcpConfig::default();
    /// let server = TcpEchoServer::new(config);
    /// ```
    pub fn new(config: TcpConfig) -> Self {
        let (shutdown_signal, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            shutdown_signal: Arc::new(shutdown_signal),
        }
    }

    /// Handles a single TCP connection with configurable timeouts
    async fn handle_connection(
        mut socket: TcpStream,
        addr: SocketAddr,
        config: TcpConfig,
    ) -> Result<()> {
        let mut buffer = vec![0; config.buffer_size];

        loop {
            // Read with timeout
            let read_result = timeout(config.read_timeout, socket.read(&mut buffer)).await;
            let n = match read_result {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    return Err(EchoError::Tcp(e));
                }
                Err(_) => {
                    warn!(%addr, "Read timeout");
                    break;
                }
            };

            if n == 0 {
                // Connection closed by client
                info!(%addr, "Client closed connection");
                break;
            }

            let preview = String::from_utf8_lossy(&buffer[..n]);
            info!(%addr, size = n, preview = %preview, "Received data");

            // Echo back the received data with optional timeout
            let write_result = timeout(config.write_timeout, async {
                socket.write_all(&buffer[..n]).await?;
                socket.flush().await?;
                Ok::<(), std::io::Error>(())
            }).await;

            match write_result {
                Ok(Ok(())) => {
                    info!(%addr, size = n, "Echoed data");
                }
                Ok(Err(e)) => {
                    return Err(EchoError::Tcp(e));
                }
                Err(_) => {
                    warn!(%addr, "Write timeout");
                    break;
                }
            }
        }

        Ok(())
    }
}

impl EchoServer for TcpEchoServer {
    /// Starts the TCP echo server and listens for connections
    async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr)
            .await
            .map_err(|e| EchoError::Config(format!("Failed to bind to {}: {}", self.config.bind_addr, e)))?;

        info!(address = %self.config.bind_addr, "TCP echo server listening");

        let connection_count = Arc::new(AtomicUsize::new(0));
        let mut shutdown_rx = self.shutdown_signal.subscribe();

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((socket, addr)) => {
                            let current_count = connection_count.load(Ordering::SeqCst);
                            if current_count >= self.config.max_connections {
                                warn!(%addr, current = current_count, limit = self.config.max_connections, "Connection rejected: limit reached");
                                continue;
                            }

                            connection_count.fetch_add(1, Ordering::SeqCst);
                            let new_count = connection_count.load(Ordering::SeqCst);
                            info!(%addr, current = new_count, "Accepted connection");

                            let config = self.config.clone();
                            let connection_count = connection_count.clone();
                            let span = tracing::info_span!("connection", %addr, current = new_count);
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(socket, addr, config).instrument(span).await {
                                    error!(%addr, error = %e, "Error handling connection");
                                }
                                let final_count = connection_count.fetch_sub(1, Ordering::SeqCst) - 1;
                                info!(%addr, current = final_count, "Connection closed");
                            });
                        }
                        Err(e) => {
                            error!(error = %e, "Failed to accept connection");
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

        info!("TCP echo server stopped");
        Ok(())
    }

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    ///
    /// # Examples
    ///
    /// ```
/// use echosrv::tcp::{TcpConfig, TcpEchoServer};
/// use echosrv::common::EchoServer;
///
/// let config = TcpConfig::default();
/// let server = TcpEchoServer::new(config);
/// let shutdown_signal = server.shutdown_signal();
/// assert_eq!(shutdown_signal.receiver_count(), 0);
/// ```
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_signal.as_ref().clone()
    }
} 