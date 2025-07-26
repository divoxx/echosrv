use crate::{Result, EchoError};
use crate::common::EchoClient;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{timeout, Duration};

/// UDP test client for the echo server
///
/// # Examples
///
/// Basic client usage:
///
/// ```no_run
/// use echosrv::udp::UdpEchoClient;
/// use echosrv::common::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = UdpEchoClient::connect(addr).await?;
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
/// use echosrv::udp::UdpEchoClient;
/// use echosrv::common::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = UdpEchoClient::connect(addr).await?;
///     
///     let data = vec![0x01, 0x02, 0x03, 0xFF];
///     let response = client.echo(&data).await?;
///     assert_eq!(response, data);
///     Ok(())
/// }
/// ```
pub struct UdpEchoClient {
    socket: UdpSocket,
    server_addr: SocketAddr,
}

impl UdpEchoClient {
    /// Connects to a UDP echo server at the given address
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use echosrv::udp::UdpEchoClient;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let addr = "127.0.0.1:8080".parse()?;
    ///     let client = UdpEchoClient::connect(addr).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn connect(server_addr: SocketAddr) -> Result<Self> {
        // Bind to any available port
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|e| EchoError::Config(format!("Failed to bind UDP socket: {}", e)))?;
        
        Ok(Self { socket, server_addr })
    }
}

impl EchoClient for UdpEchoClient {
    /// Sends data to the UDP echo server and returns the echoed response.
    /// Waits for a single response datagram.
    ///
    /// # Examples
    ///
    /// ```no_run
/// use echosrv::udp::UdpEchoClient;
/// use echosrv::common::EchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = UdpEchoClient::connect(addr).await?;
///     let data = b"Hello, Server!";
///     let response = client.echo(data).await?;
///     assert_eq!(response, data);
///     Ok(())
/// }
/// ```
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        // Send the data
        self.socket.send_to(data, self.server_addr).await
            .map_err(|e| EchoError::Udp(e))?;

        // Receive the echo response
        let mut buffer = vec![0; 1024];
        let (n, _) = timeout(Duration::from_millis(500), self.socket.recv_from(&mut buffer)).await
            .map_err(|_| EchoError::Timeout("UDP receive timeout".to_string()))?
            .map_err(|e| EchoError::Udp(e))?;

        Ok(buffer[..n].to_vec())
    }
} 