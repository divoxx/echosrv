use super::{DatagramConfig, DatagramProtocol};
use crate::common::EchoClient;
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::time::{Duration, timeout};

/// Generic datagram-based echo client that works with any datagram protocol
///
/// This client can work with any protocol that implements `DatagramProtocol`,
/// such as UDP, Unix datagrams, etc.
///
/// # Examples
///
/// ```no_run
/// use echosrv::datagram::{DatagramConfig, DatagramEchoClient};
/// use echosrv::common::EchoClient;
/// use echosrv::udp::UdpProtocol;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client: DatagramEchoClient<UdpProtocol> = DatagramEchoClient::connect(addr).await?;
///     
///     let response = client.echo_string("Hello, World!").await?;
///     println!("Echo response: {}", response);
///     Ok(())
/// }
/// ```
pub struct DatagramEchoClient<P: DatagramProtocol> {
    socket: P::Socket,
    server_addr: SocketAddr,
}

impl<P: DatagramProtocol> DatagramEchoClient<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Connects to a datagram echo server at the given address
    pub async fn connect(server_addr: SocketAddr) -> Result<Self> {
        // For datagram protocols, we need to bind to any available address
        let config = DatagramConfig {
            bind_addr: "0.0.0.0:0".parse().unwrap(),
            buffer_size: 1024,
            read_timeout: std::time::Duration::from_secs(30),
            write_timeout: std::time::Duration::from_secs(30),
        };

        let socket = P::bind(&config).await.map_err(|e| e.into())?;

        Ok(Self {
            socket,
            server_addr,
        })
    }
}

#[async_trait]
impl<P: DatagramProtocol> EchoClient for DatagramEchoClient<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Sends data to the echo server and returns the echoed response
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        // Send data to server
        P::send_to(&self.socket, data, self.server_addr)
            .await
            .map_err(|e| e.into())?;

        // Receive response with timeout
        let mut buffer = vec![0; 1024];
        let (n, _) = timeout(
            Duration::from_millis(500),
            P::recv_from(&self.socket, &mut buffer),
        )
        .await
        .map_err(|_| EchoError::Timeout("Datagram receive timeout".to_string()))?
        .map_err(|e| e.into())?;

        Ok(buffer[..n].to_vec())
    }
}
