use crate::{Result, EchoError};
use crate::common::EchoClient;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{timeout, Duration},
};

/// TCP test client for the echo server
///
/// # Examples
///
/// Basic client usage:
///
/// ```no_run
/// use echosrv::tcp::TcpEchoClient;
/// use echosrv::common::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = TcpEchoClient::connect(addr).await?;
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
/// use echosrv::tcp::TcpEchoClient;
/// use echosrv::common::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = TcpEchoClient::connect(addr).await?;
///     
///     let data = vec![0x01, 0x02, 0x03, 0xFF];
///     let response = client.echo(&data).await?;
///     assert_eq!(response, data);
///     Ok(())
/// }
/// ```
pub struct TcpEchoClient {
    stream: TcpStream,
}

impl TcpEchoClient {
    /// Connects to a TCP echo server at the given address
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use echosrv::tcp::TcpEchoClient;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let addr = "127.0.0.1:8080".parse()?;
    ///     let client = TcpEchoClient::connect(addr).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| EchoError::Config(format!("Failed to connect to {}: {}", addr, e)))?;
        Ok(Self { stream })
    }
}

impl EchoClient for TcpEchoClient {
    /// Sends data to the TCP echo server and returns the echoed response.
    /// Reads in a loop until the connection is closed or a read times out.
    ///
    /// # Examples
    ///
    /// ```no_run
/// use echosrv::tcp::TcpEchoClient;
/// use echosrv::common::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = TcpEchoClient::connect(addr).await?;
///     
///     let data = b"Hello, Server!";
///     let response = client.echo(data).await?;
///     assert_eq!(response, data);
///     Ok(())
/// }
/// ```
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {

        self.stream.write_all(data).await.map_err(EchoError::Tcp)?;
        self.stream.flush().await.map_err(EchoError::Tcp)?;

        let mut response = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            match timeout(Duration::from_millis(200), self.stream.read(&mut buffer)).await {
                Ok(Ok(0)) => break, // Connection closed
                Ok(Ok(n)) => response.extend_from_slice(&buffer[..n]),
                Ok(Err(e)) => return Err(EchoError::Tcp(e)),
                Err(_) => break, // Timeout, assume done
            }
        }
        Ok(response)
    }
} 