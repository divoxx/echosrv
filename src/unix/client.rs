use crate::Result;
use crate::common::EchoClient;
use crate::unix::datagram_protocol::{UnixDatagramExt, UnixDatagramProtocol};
use crate::unix::stream_protocol::{ManagedUnixStream, Protocol, StreamExt};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixDatagram;

/// Unix domain stream echo client
///
/// This client connects to Unix domain stream servers and can send
/// data to be echoed back. It's optimized for inter-process communication
/// on Unix-like systems.
///
/// # Examples
///
/// ```no_run
/// use echosrv::unix::UnixStreamEchoClient;
/// use echosrv::common::EchoClient;
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let socket_path = PathBuf::from("/tmp/echo.sock");
///     let mut client = UnixStreamEchoClient::connect(socket_path).await?;
///     
///     let response = client.echo_string("Hello, Unix Stream Server!").await?;
///     println!("Server echoed: {}", response);
///     Ok(())
/// }
/// ```
pub struct UnixStreamEchoClient {
    stream: ManagedUnixStream,
}

impl UnixStreamEchoClient {
    /// Connects to a Unix domain stream echo server at the given socket path
    pub async fn connect(socket_path: PathBuf) -> Result<Self> {
        let stream = Protocol::connect_unix(&socket_path).await?;
        Ok(Self { stream })
    }
}

#[async_trait]
impl EchoClient for UnixStreamEchoClient {
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        // Send the data
        self.stream
            .write_all(data)
            .await
            .map_err(crate::EchoError::Unix)?;
        self.stream.flush().await.map_err(crate::EchoError::Unix)?;

        // Read the echoed response
        let mut buffer = vec![0u8; data.len()];
        let mut response = Vec::new();
        let mut total_read = 0;

        while total_read < data.len() {
            let n = self
                .stream
                .read(&mut buffer)
                .await
                .map_err(crate::EchoError::Unix)?;
            if n == 0 {
                break; // Connection closed
            }
            response.extend_from_slice(&buffer[..n]);
            total_read += n;
        }

        Ok(response)
    }
}

/// Unix domain datagram echo client
///
/// This client connects to Unix domain datagram servers and can send
/// data to be echoed back. It's optimized for connectionless inter-process
/// communication on Unix-like systems.
///
/// # Examples
///
/// ```no_run
/// use echosrv::unix::UnixDatagramEchoClient;
/// use echosrv::common::EchoClient;
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let socket_path = PathBuf::from("/tmp/echo_dgram.sock");
///     let mut client = UnixDatagramEchoClient::connect(socket_path).await?;
///     
///     let response = client.echo_string("Hello, Unix Datagram Server!").await?;
///     println!("Server echoed: {}", response);
///     Ok(())
/// }
/// ```
pub struct UnixDatagramEchoClient {
    socket: UnixDatagram,
    server_path: PathBuf,
}

impl UnixDatagramEchoClient {
    /// Connects to a Unix domain datagram echo server at the given socket path
    pub async fn connect(server_path: PathBuf) -> Result<Self> {
        let socket = UnixDatagramProtocol::connect_unix(&server_path).await?;
        Ok(Self {
            socket,
            server_path,
        })
    }
}

#[async_trait]
impl EchoClient for UnixDatagramEchoClient {
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        // Send the data to the server
        self.socket
            .send_to(data, &self.server_path)
            .await
            .map_err(crate::EchoError::Unix)?;

        // Receive the echoed response
        let mut buffer = vec![0u8; data.len()];
        let (len, _) = self
            .socket
            .recv_from(&mut buffer)
            .await
            .map_err(crate::EchoError::Unix)?;

        Ok(buffer[..len].to_vec())
    }
}
