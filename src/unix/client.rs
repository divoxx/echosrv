//! Unix domain stream and datagram echo clients.

use crate::Result;
use crate::common::EchoClient;
use crate::datagram::DatagramClientConfig;
use crate::datagram::client::{finish_reply, reply_buffer};
use crate::unix::datagram_protocol::{ManagedUnixDatagram, UnixDatagramExt, UnixDatagramProtocol};
use crate::unix::stream_protocol::UnixStreamProtocol;
use crate::{EchoError, stream::Client};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::time::timeout;

/// Unix domain stream echo client.
///
/// An alias for the generic stream [`Client`], so it shares its connect/read/
/// write timeouts and response size limit
/// ([`ClientConfig`](crate::stream::ClientConfig)).
///
/// # Examples
///
/// ```
/// use echosrv::unix::UnixStreamEchoClient;
/// use echosrv::EchoClient;
/// # use echosrv::unix::{UnixStreamConfig, UnixStreamEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let dir = tempfile::tempdir()?;
/// # let socket_path = dir.path().join("echo.sock");
/// # let config = UnixStreamConfig::default().with_socket_path(socket_path.clone());
/// # let server = UnixStreamEchoServer::new(config);
/// # tokio::spawn(server.bind().await?.serve());
/// let mut client = UnixStreamEchoClient::connect(socket_path).await?;
/// assert_eq!(client.echo_string("Hello").await?, "Hello");
/// # Ok(())
/// # }
/// ```
pub type UnixStreamEchoClient = Client<UnixStreamProtocol>;

/// Unix domain datagram echo client.
///
/// Binds a temporary socket path in [`std::env::temp_dir`] (so the server can
/// reply), which is removed when the client is dropped. Each
/// [`echo`](EchoClient::echo) sends one datagram and returns the next datagram
/// received (default timeouts 5 s, 64 KiB receive buffer; see
/// [`DatagramClientConfig`]).
///
/// Errors: a missing server socket is an [`EchoError::Unix`] with
/// [`NotFound`](std::io::ErrorKind::NotFound) (a stale socket file with no
/// server gives `ConnectionRefused`); no reply within `read_timeout` is an
/// [`EchoError::Timeout`]; a reply larger than `buffer_size` is an
/// [`EchoError::Config`] rather than a truncated echo.
///
/// # Examples
///
/// ```
/// use echosrv::unix::UnixDatagramEchoClient;
/// use echosrv::EchoClient;
/// # use echosrv::unix::{UnixDatagramConfig, UnixDatagramEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let dir = tempfile::tempdir()?;
/// # let socket_path = dir.path().join("echo.sock");
/// # let config = UnixDatagramConfig::default().with_socket_path(socket_path.clone());
/// # let server = UnixDatagramEchoServer::new(config);
/// # tokio::spawn(server.bind().await?.serve());
/// let mut client = UnixDatagramEchoClient::connect(socket_path).await?;
/// assert_eq!(client.echo_string("Hello").await?, "Hello");
/// # Ok(())
/// # }
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

        let mut buffer = reply_buffer(self.config.buffer_size);
        let (len, _) = timeout(
            self.config.read_timeout,
            self.socket.get_ref().recv_from(&mut buffer),
        )
        .await
        .map_err(|_| EchoError::Timeout("Datagram receive timeout".to_string()))?
        .map_err(EchoError::Unix)?;

        finish_reply(buffer, len, self.config.buffer_size)
    }
}
