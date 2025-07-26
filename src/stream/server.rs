use crate::{Result, EchoError};
use crate::common::EchoServerTrait;
use super::{StreamConfig, StreamProtocol};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{
    signal,
    time::timeout,
};
use tracing::{error, info, warn, Instrument};
use async_trait::async_trait;

/// Generic stream-based echo server that works with any stream protocol
///
/// This server can work with any protocol that implements `StreamProtocol`,
/// such as TCP, Unix streams, etc.
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
///     };
///
///     let server: StreamEchoServer<TcpProtocol> = StreamEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
pub struct StreamEchoServer<P: StreamProtocol> {
    config: StreamConfig,
    protocol: std::marker::PhantomData<P>,
    shutdown_signal: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl<P: StreamProtocol> StreamEchoServer<P> 
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Creates a new stream-based echo server with the given configuration
    pub fn new(config: StreamConfig) -> Self {
        let (shutdown_signal, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            protocol: std::marker::PhantomData,
            shutdown_signal: Arc::new(shutdown_signal),
        }
    }
    
    /// Handles a single stream-based connection
    async fn handle_connection(
        mut stream: P::Stream,
        addr: SocketAddr,
        config: StreamConfig,
    ) -> Result<()> {
        let mut buffer = vec![0; config.buffer_size];

        loop {
            // Read with timeout
            let read_result = timeout(config.read_timeout, P::read(&mut stream, &mut buffer)).await;
            let n = match read_result {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    return Err(e.into());
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

            // Echo back the received data with timeout
            let write_result = timeout(config.write_timeout, P::write(&mut stream, &buffer[..n])).await;
            match write_result {
                Ok(Ok(())) => {
                    P::flush(&mut stream).await.map_err(|e| e.into())?;
                    info!(%addr, size = n, "Echoed data");
                }
                Ok(Err(e)) => {
                    return Err(e.into());
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

#[async_trait]
impl<P: StreamProtocol + Sync> EchoServerTrait for StreamEchoServer<P> 
where
    P::Error: Into<EchoError> + std::fmt::Display,
    P::Stream: 'static,
{
        /// Starts the stream-based echo server and listens for connections
    async fn run(&self) -> Result<()> {
        let mut listener = P::bind(&self.config).await.map_err(|e| e.into())?;

        info!(address = %self.config.bind_addr, "Stream echo server listening");

        let connection_count = Arc::new(AtomicUsize::new(0));
        let mut shutdown_rx = self.shutdown_signal.subscribe();

        loop {
            tokio::select! {
                accept_result = P::accept(&mut listener) => {
                    match accept_result {
                        Ok((stream, addr)) => {
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
                            
                            // Handle connection in a separate task with proper Send bounds
                            tokio::spawn(async move {
                                let result = Self::handle_connection(stream, addr, config).instrument(span).await;
                                if let Err(e) = result {
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

        info!("Stream echo server stopped");
        Ok(())
    }

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_signal.as_ref().clone()
    }
} 