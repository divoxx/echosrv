use color_eyre::eyre::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    signal,
    time::timeout,
};
use tracing::{error, info, warn, Instrument};

/// Configuration for the TCP echo server
///
/// # Examples
///
/// ```
/// use echosrv::tcp::Config;
/// use std::time::Duration;
///
/// let config = Config {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     max_connections: 100,
///     buffer_size: 1024,
///     read_timeout: Some(Duration::from_secs(30)),
///     write_timeout: Some(Duration::from_secs(30)),
/// };
/// ```
///
/// Using the default configuration:
///
/// ```
/// use echosrv::tcp::Config;
///
/// let config = Config::default();
/// assert_eq!(config.max_connections, 100);
/// assert_eq!(config.buffer_size, 1024);
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind the server to
    pub bind_addr: SocketAddr,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections (None for no timeout)
    pub read_timeout: Option<Duration>,
    /// Write timeout for connections (None for no timeout)
    pub write_timeout: Option<Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(), // Use port 0 for testing
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Some(Duration::from_secs(30)),
            write_timeout: Some(Duration::from_secs(30)),
        }
    }
}

/// TCP echo server that handles TCP connections
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::tcp::{Config, EchoServer};
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = Config {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         max_connections: 100,
///         buffer_size: 1024,
///         read_timeout: Some(Duration::from_secs(30)),
///         write_timeout: Some(Duration::from_secs(30)),
///     };
///
///     let server = EchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Server with graceful shutdown:
///
/// ```no_run
/// use echosrv::tcp::{Config, EchoServer};
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = Config::default();
///     let server = EchoServer::new(config);
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
pub struct EchoServer {
    config: Config,
    shutdown_signal: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl EchoServer {
    /// Creates a new TCP echo server with the given configuration
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::tcp::{Config, EchoServer};
    ///
    /// let config = Config::default();
    /// let server = EchoServer::new(config);
    /// ```
    pub fn new(config: Config) -> Self {
        let (shutdown_signal, _) = tokio::sync::broadcast::channel(1);
        Self {
            config,
            shutdown_signal: Arc::new(shutdown_signal),
        }
    }

    /// Returns a shutdown signal sender that can be used to gracefully shutdown the server
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::tcp::{Config, EchoServer};
    ///
    /// let config = Config::default();
    /// let server = EchoServer::new(config);
    /// let shutdown_signal = server.shutdown_signal();
    /// assert_eq!(shutdown_signal.receiver_count(), 0);
    /// ```
    pub fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()> {
        self.shutdown_signal.as_ref().clone()
    }

    /// Starts the TCP echo server and listens for connections
    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr)
            .await
            .with_context(|| format!("Failed to bind to {}", self.config.bind_addr))?;

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

    /// Handles a single TCP connection with configurable timeouts
    async fn handle_connection(
        mut socket: TcpStream,
        addr: SocketAddr,
        config: Config,
    ) -> Result<()> {
        let mut buffer = vec![0; config.buffer_size];

        loop {
            // Read with optional timeout
            let read_result = if let Some(timeout_duration) = config.read_timeout {
                timeout(timeout_duration, socket.read(&mut buffer)).await
            } else {
                Ok(socket.read(&mut buffer).await)
            };

            let n = match read_result {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    return Err(e).with_context(|| format!("Failed to read from connection {}", addr));
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
            let write_result = if let Some(timeout_duration) = config.write_timeout {
                timeout(timeout_duration, async {
                    socket.write_all(&buffer[..n]).await?;
                    socket.flush().await?;
                    Ok::<(), std::io::Error>(())
                }).await
            } else {
                Ok(async {
                    socket.write_all(&buffer[..n]).await?;
                    socket.flush().await?;
                    Ok::<(), std::io::Error>(())
                }.await)
            };

            match write_result {
                Ok(Ok(())) => {
                    info!(%addr, size = n, "Echoed data");
                }
                Ok(Err(e)) => {
                    return Err(color_eyre::eyre::eyre!("Failed to write to connection {}: {}", addr, e));
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

/// TCP test client for the echo server
///
/// # Examples
///
/// Basic client usage:
///
/// ```no_run
/// use echosrv::tcp::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = EchoClient::connect(addr).await?;
///     
///     let response = client.echo_string("Hello, Server!").await?;
///     println!("Server echoed: {}", response);
///     Ok(())
/// }
/// ```
///
/// Sending binary data:
///
/// ```no_run
/// use echosrv::tcp::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = EchoClient::connect(addr).await?;
///     
///     let data = vec![0x01, 0x02, 0x03, 0xFF];
///     let response = client.echo(&data).await?;
///     assert_eq!(response, data);
///     Ok(())
/// }
/// ```
pub struct EchoClient {
    stream: TcpStream,
}

impl EchoClient {
    /// Connects to a TCP echo server at the given address
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use echosrv::tcp::EchoClient;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let addr = "127.0.0.1:8080".parse()?;
    ///     let client = EchoClient::connect(addr).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("Failed to connect to {}", addr))?;
        Ok(Self { stream })
    }

    /// Sends data to the TCP echo server and returns the echoed response.
    /// Reads in a loop until the connection is closed or a read times out.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use echosrv::tcp::EchoClient;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let addr = "127.0.0.1:8080".parse()?;
    ///     let mut client = EchoClient::connect(addr).await?;
    ///     
    ///     let data = b"Hello, Server!";
    ///     let response = client.echo(data).await?;
    ///     assert_eq!(response, data);
    ///     Ok(())
    /// }
    /// ```
    pub async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        use tokio::io::AsyncWriteExt;
        use tokio::time::{timeout, Duration};
        self.stream.write_all(data).await?;
        self.stream.flush().await?;

        let mut response = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match timeout(Duration::from_millis(200), self.stream.read(&mut buffer)).await {
                Ok(Ok(0)) => break, // Connection closed
                Ok(Ok(n)) => response.extend_from_slice(&buffer[..n]),
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => break, // Timeout, assume done
            }
        }
        Ok(response)
    }

    /// Sends a string and returns the echoed string
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use echosrv::tcp::EchoClient;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let addr = "127.0.0.1:8080".parse()?;
    ///     let mut client = EchoClient::connect(addr).await?;
    ///     
    ///     let message = "Hello, Echo Server!";
    ///     let response = client.echo_string(message).await?;
    ///     assert_eq!(response, message);
    ///     Ok(())
    /// }
    /// ```
    pub async fn echo_string(&mut self, data: &str) -> Result<String> {
        let response = self.echo(data.as_bytes()).await?;
        String::from_utf8(response).map_err(|e| color_eyre::eyre::eyre!("Invalid UTF-8: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Helper function to create a simple test server
    async fn create_test_server() -> Result<(tokio::task::JoinHandle<Result<()>>, SocketAddr)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let server_handle = tokio::spawn(async move {
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(5),
                    listener.accept()
                ).await {
                    Ok(Ok((socket, addr))) => {
                        tokio::spawn(async move {
                            if let Err(e) = handle_test_connection(socket, addr).await {
                                error!("Test server: Error handling connection from {}: {}", addr, e);
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        error!("Test server: Failed to accept connection: {}", e);
                        break;
                    }
                    Err(_) => {
                        // Timeout - server is done
                        break;
                    }
                }
            }
            info!("Test server stopped");
            Ok(())
        });
        Ok((server_handle, addr))
    }

    /// Simple connection handler for tests
    async fn handle_test_connection(
        mut socket: TcpStream,
        addr: SocketAddr,
    ) -> Result<()> {
        let mut buffer = [0; 1024];

        loop {
            let n = socket
                .read(&mut buffer)
                .await
                .with_context(|| format!("Failed to read from connection {}", addr))?;

            if n == 0 {
                // Connection closed by client
                break;
            }

            // Echo back the received data
            socket
                .write_all(&buffer[..n])
                .await
                .with_context(|| format!("Failed to write to connection {}", addr))?;

            socket
                .flush()
                .await
                .with_context(|| format!("Failed to flush connection {}", addr))?;

            info!("Test server: Echoed {} bytes to {}", n, addr);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.buffer_size, 1024);
        assert!(config.read_timeout.is_some());
        assert!(config.write_timeout.is_some());
    }

    #[tokio::test]
    async fn test_echo_server_new() {
        let config = Config::default();
        let server = EchoServer::new(config);
        assert!(server.shutdown_signal().receiver_count() == 0);
    }

    #[tokio::test]
    async fn test_echo_client_connect() {
        let (server_handle, addr) = create_test_server().await.unwrap();
        
        // Test client connection
        let client = EchoClient::connect(addr).await;
        assert!(client.is_ok());
        
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_basic_echo() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Test basic echo functionality
        let mut client = EchoClient::connect(addr).await?;
        
        let test_data = "Hello, Echo Server!";
        let response = client.echo_string(test_data).await?;
        
        assert_eq!(response, test_data);
        
        // Clean shutdown
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_messages() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = EchoClient::connect(addr).await?;
        
        // Test multiple messages
        let messages = vec![
            "First message",
            "Second message",
            "Third message with special chars: !@#$%^&*()",
            "Unicode: 🚀🌟🎉",
            "",
        ];

        for message in messages {
            let response = client.echo_string(message).await?;
            assert_eq!(response, message);
        }
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_binary_data() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = EchoClient::connect(addr).await?;
        
        // Test binary data
        let binary_data = vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0xFD];
        let response = client.echo(&binary_data).await?;
        
        assert_eq!(response, binary_data);
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_large_data() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = EchoClient::connect(addr).await?;
        
        // Test large data (larger than buffer size)
        let large_data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let response = client.echo(&large_data).await?;
        
        assert_eq!(response, large_data);
        
        server_handle.abort();
        Ok(())
    }
} 