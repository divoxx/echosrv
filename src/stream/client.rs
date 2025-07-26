use crate::{Result, EchoError};
use crate::common::EchoClient;
use super::StreamProtocol;
use std::net::SocketAddr;
use tokio::time::{timeout, Duration};

/// Generic stream-based echo client that works with any stream protocol
///
/// This client can work with any protocol that implements `StreamProtocol`,
/// such as TCP, Unix streams, etc.
///
/// # Examples
///
/// Using with TCP protocol:
///
/// ```no_run
/// use echosrv::stream::StreamEchoClient;
/// use echosrv::common::EchoClient;
/// use echosrv::tcp::TcpProtocol;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client: StreamEchoClient<TcpProtocol> = StreamEchoClient::connect(addr).await?;
///     
///     let response = client.echo_string("Hello, World!").await?;
///     println!("Echo response: {}", response);
///     Ok(())
/// }
/// ```
///
/// Using with binary data:
///
/// ```no_run
/// use echosrv::stream::StreamEchoClient;
/// use echosrv::common::EchoClient;
/// use echosrv::tcp::TcpProtocol;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client: StreamEchoClient<TcpProtocol> = StreamEchoClient::connect(addr).await?;
///     
///     let data = vec![0x01, 0x02, 0x03, 0xFF];
///     let response = client.echo(&data).await?;
///     assert_eq!(response, data);
///     Ok(())
/// }
/// ```
pub struct StreamEchoClient<P: StreamProtocol> {
    stream: P::Stream,
}

impl<P: StreamProtocol> StreamEchoClient<P> 
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Connects to a stream echo server at the given address
    pub async fn connect(server_addr: SocketAddr) -> Result<Self> {
        let stream = P::connect(server_addr).await.map_err(|e| e.into())?;
        Ok(Self { stream })
    }
}

impl<P: StreamProtocol> EchoClient for StreamEchoClient<P> 
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Sends data to the echo server and returns the echoed response
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        // Write the data
        P::write(&mut self.stream, data).await.map_err(|e| e.into())?;
        P::flush(&mut self.stream).await.map_err(|e| e.into())?;

        // Read the response
        let mut response = Vec::new();
        let mut buffer = [0; 1024];
        
        loop {
            match timeout(Duration::from_millis(200), P::read(&mut self.stream, &mut buffer)).await {
                Ok(Ok(0)) => break, // Connection closed
                Ok(Ok(n)) => response.extend_from_slice(&buffer[..n]),
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => break, // Timeout, assume done
            }
        }
        
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcp::TcpProtocol;
    use crate::tcp::{TcpConfig, TcpEchoServer};
    use crate::common::traits::EchoServerTrait;
    use std::time::Duration;

    #[tokio::test]
    async fn test_stream_client_connect() {
        let config = TcpConfig::default();
        let server = TcpEchoServer::new(config.into());
        assert!(server.shutdown_signal().receiver_count() == 0);
    }

    #[tokio::test]
    async fn test_stream_client_with_tcp_protocol() -> Result<()> {
        // First bind to get the actual address
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await
            .map_err(|e| EchoError::Config(format!("Failed to bind listener: {}", e)))?;
        let addr = listener.local_addr()
            .map_err(|e| EchoError::Config(format!("Failed to get local address: {}", e)))?;
        drop(listener); // Close the listener so the server can bind to the same address
        
        // Start a TCP server
        let config = TcpConfig {
            bind_addr: addr,
            max_connections: 10,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        };
        
        let server = TcpEchoServer::new(config.into());
        
        // Spawn the server
        let server_handle = tokio::spawn(async move {
            server.run().await
        });

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Test the generic stream client with TCP protocol
        let mut client: StreamEchoClient<TcpProtocol> = StreamEchoClient::connect(addr).await?;
        
        let test_data = "Hello from generic stream client!";
        let response = client.echo_string(test_data).await?;
        
        assert_eq!(response, test_data);
        
        // Clean shutdown
        server_handle.abort();
        Ok(())
    }
} 