use crate::Result;
use crate::common::EchoServerTrait;
use crate::unix::config::{UnixDatagramConfig, UnixStreamConfig};
use crate::unix::datagram_protocol::{UnixDatagramExt, UnixDatagramProtocol};
use crate::unix::stream_protocol::{Protocol, StreamExt};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tracing::{error, info};

/// Unix domain stream echo server
///
/// This server handles Unix domain stream connections and echoes back
/// all received data. It's optimized for inter-process communication
/// on Unix-like systems.
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
///         socket_path: "/tmp/echo.sock".into(),
///         max_connections: 100,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///     };
///
///     let server = UnixStreamEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
pub struct UnixStreamEchoServer {
    config: UnixStreamConfig,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl UnixStreamEchoServer {
    /// Creates a new Unix domain stream echo server with the given configuration
    pub fn new(config: UnixStreamConfig) -> Self {
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            shutdown_tx,
        }
    }
}

#[async_trait]
impl EchoServerTrait for UnixStreamEchoServer {
    async fn run(&self) -> Result<()> {
        let socket_path = &self.config.socket_path;

        info!(
            "Starting Unix domain stream echo server on {}",
            socket_path.display()
        );

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);

        let mut listener = Protocol::bind_unix(socket_path).await?;
        info!(
            "Unix domain stream server bound to {}",
            socket_path.display()
        );

        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((mut stream, _addr)) => {
                            let buffer_size = self.config.buffer_size;
                            let read_timeout = self.config.read_timeout;
                            let write_timeout = self.config.write_timeout;

                            // Spawn a task to handle this connection
                            tokio::spawn(async move {
                                let mut buffer = vec![0u8; buffer_size];

                                loop {
                                    // Read with timeout
                                    let read_result = timeout(read_timeout, stream.read(&mut buffer)).await;
                                    match read_result {
                                        Ok(Ok(0)) => {
                                            // Connection closed by client
                                            break;
                                        }
                                        Ok(Ok(n)) => {
                                            let data = &buffer[..n];

                                            // Echo back with timeout
                                            if let Err(e) = timeout(write_timeout, stream.write_all(data)).await {
                                                error!("Write timeout or error: {}", e);
                                                break;
                                            }

                                            if let Err(e) = timeout(write_timeout, stream.flush()).await {
                                                error!("Flush timeout or error: {}", e);
                                                break;
                                            }
                                        }
                                        Ok(Err(e)) => {
                                            error!("Read error: {}", e);
                                            break;
                                        }
                                        Err(_) => {
                                            error!("Read timeout");
                                            break;
                                        }
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("Accept error: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received, stopping Unix domain stream server");
                    break;
                }
            }
        }

        // Clean up socket file
        let _ = std::fs::remove_file(socket_path);
        info!("Unix domain stream server stopped");
        Ok(())
    }

    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_tx.clone()
    }
}

/// Unix domain datagram echo server
///
/// This server handles Unix domain datagram messages and echoes back
/// all received data. It's optimized for connectionless inter-process
/// communication on Unix-like systems.
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
///         socket_path: "/tmp/echo_dgram.sock".into(),
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///     };
///
///     let server = UnixDatagramEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
pub struct UnixDatagramEchoServer {
    config: UnixDatagramConfig,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl UnixDatagramEchoServer {
    /// Creates a new Unix domain datagram echo server with the given configuration
    pub fn new(config: UnixDatagramConfig) -> Self {
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            shutdown_tx,
        }
    }
}

#[async_trait]
impl EchoServerTrait for UnixDatagramEchoServer {
    async fn run(&self) -> Result<()> {
        let socket_path = &self.config.socket_path;

        info!(
            "Starting Unix domain datagram echo server on {}",
            socket_path.display()
        );

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);

        let socket = UnixDatagramProtocol::bind_unix(socket_path).await?;
        info!(
            "Unix domain datagram server bound to {}",
            socket_path.display()
        );
        info!("Server socket created successfully");

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut buffer = vec![0u8; self.config.buffer_size];

        loop {
            tokio::select! {
                recv_result = socket.recv_from(&mut buffer) => {
                    match recv_result {
                        Ok((len, peer_addr)) => {
                            let data = &buffer[..len];
                            info!("Received {} bytes from peer", len);

                            // Echo back to the same peer
                            // For Unix datagrams, we need to convert the address to a path
                            if let Some(path) = peer_addr.as_pathname() {
                                if let Err(e) = socket.send_to(data, path).await {
                                    error!("Failed to send response: {}", e);
                                } else {
                                    info!("Sent {} bytes back to peer", len);
                                }
                            } else {
                                // For anonymous sockets, we can't reply because we don't have a path
                                // The client should use a named socket if it wants to receive responses
                                error!("Received message from unnamed socket, cannot reply. Client should use a named socket.");
                            }
                        }
                        Err(e) => {
                            error!("Receive error: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received, stopping Unix domain datagram server");
                    break;
                }
            }
        }

        // Clean up socket file
        let _ = std::fs::remove_file(socket_path);
        info!("Unix domain datagram server stopped");
        Ok(())
    }

    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_tx.clone()
    }
}
