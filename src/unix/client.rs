use crate::Result;
use crate::common::EchoClient;
use crate::datagram::DatagramClientConfig;
use crate::unix::datagram_protocol::{ManagedUnixDatagram, UnixDatagramExt, UnixDatagramProtocol};
use crate::unix::stream_protocol::UnixStreamProtocol;
use crate::{EchoError, stream::Client};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::time::timeout;

/// Unix domain stream echo client
///
/// An alias for the generic stream [`Client`], so it shares its connect/read/
/// write timeouts and response size limit
/// ([`ClientConfig`](crate::stream::ClientConfig)).
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
pub type UnixStreamEchoClient = Client<UnixStreamProtocol>;

/// Unix domain datagram echo client
///
/// Binds a temporary socket path (so the server can reply), which is removed
/// when the client is dropped.
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
    socket: ManagedUnixDatagram,
    server_path: PathBuf,
    config: DatagramClientConfig,
}

impl UnixDatagramEchoClient {
    /// Creates a client for the server at `server_path` with default settings
    /// (64 KiB receive buffer, 5 s timeouts).
    pub async fn connect(server_path: PathBuf) -> Result<Self> {
        Self::connect_with_config(server_path, DatagramClientConfig::default()).await
    }

    /// Creates a client for the server at `server_path` with custom settings.
    pub async fn connect_with_config(
        server_path: PathBuf,
        config: DatagramClientConfig,
    ) -> Result<Self> {
        if config.buffer_size == 0 {
            return Err(EchoError::Config(
                "buffer_size must be greater than 0".into(),
            ));
        }
        let socket = UnixDatagramProtocol::create_client_socket().await?;
        Ok(Self {
            socket,
            server_path,
            config,
        })
    }

    /// Path of the temporary socket this client receives replies on.
    pub fn local_path(&self) -> Option<&std::path::Path> {
        self.socket.owned_socket_path()
    }
}

#[async_trait]
impl EchoClient for UnixDatagramEchoClient {
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        timeout(
            self.config.write_timeout,
            self.socket.get_ref().send_to(data, &self.server_path),
        )
        .await
        .map_err(|_| EchoError::Timeout("Datagram send timeout".to_string()))?
        .map_err(EchoError::Unix)?;

        let mut buffer = vec![0u8; self.config.buffer_size];
        let (len, _) = timeout(
            self.config.read_timeout,
            self.socket.get_ref().recv_from(&mut buffer),
        )
        .await
        .map_err(|_| EchoError::Timeout("Datagram receive timeout".to_string()))?
        .map_err(EchoError::Unix)?;

        buffer.truncate(len);
        Ok(buffer)
    }
}
