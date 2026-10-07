//! The generic stream echo client, [`Client`], and its configuration.

use super::StreamProtocol;
use crate::common::EchoClient;
use crate::network::Address;
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, timeout};

/// Configuration for stream clients ([`Client`] and
/// [`HttpEchoClient`](crate::http::HttpEchoClient)).
///
/// Defaults: 30 s read and write timeouts, 10 s connect timeout, 1 KiB read
/// buffer, 10 MiB maximum response size.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Timeout for each individual read.
    pub read_timeout: Duration,
    /// Timeout for writing (and flushing) the whole request.
    pub write_timeout: Duration,
    /// Timeout for establishing the connection.
    pub connect_timeout: Duration,
    /// Size of the read buffer. A value of `0` is treated as `1`.
    pub buffer_size: usize,
    /// Largest payload/response accepted, in bytes. Larger requests and
    /// responses fail with [`EchoError::Config`].
    pub max_response_size: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            buffer_size: 1024,
            max_response_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Generic stream echo client.
///
/// [`TcpEchoClient`](crate::TcpEchoClient) and
/// [`UnixStreamEchoClient`](crate::UnixStreamEchoClient) are aliases of this
/// type. One connection is opened by `connect` and reused for every
/// [`echo`](EchoClient::echo) call.
///
/// `echo` writes the payload and, concurrently, reads until the same number
/// of bytes has come back (or the server closes the connection, in which case
/// the bytes received so far are returned). An empty payload returns an empty
/// response without touching the socket. Payloads larger than
/// [`ClientConfig::max_response_size`] are rejected with [`EchoError::Config`].
///
/// # Examples
///
/// ```
/// use echosrv::stream::ClientConfig;
/// use echosrv::tcp::{TcpConfig, TcpEchoClient};
/// use echosrv::{EchoClient, EchoServerTrait, TcpEchoServer};
/// use std::time::Duration;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// # let server = TcpEchoServer::new(TcpConfig::default().into());
/// # let bound = server.bind().await?;
/// # let addr = *bound.local_addr().as_network().unwrap();
/// # tokio::spawn(bound.serve());
/// let config = ClientConfig {
///     read_timeout: Duration::from_secs(1),
///     ..ClientConfig::default()
/// };
/// let mut client = TcpEchoClient::connect_with_config(addr, config).await?;
/// assert_eq!(client.echo(b"ping").await?, b"ping");
/// assert_eq!(client.echo(b"pong").await?, b"pong"); // same connection
/// # Ok(())
/// # }
/// ```
pub struct Client<P: StreamProtocol> {
    stream: P::Stream,
    config: ClientConfig,
    last_activity: Instant,
}

impl<P: StreamProtocol> Client<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
    P::Stream: AsyncRead + AsyncWrite + Unpin,
{
    /// Connects to a server with a custom configuration.
    ///
    /// Fails with [`EchoError::Timeout`] if the connection is not established
    /// within [`ClientConfig::connect_timeout`].
    ///
    /// `address` may be a [`SocketAddr`](std::net::SocketAddr), a
    /// [`PathBuf`](std::path::PathBuf) (Unix socket) or an [`Address`]; whether
    /// a given kind is supported depends on the protocol
    /// ([`StreamProtocol::connect_address`]).
    pub async fn connect_with_config<A: Into<Address>>(
        address: A,
        config: ClientConfig,
    ) -> Result<Self> {
        let address = address.into();
        let stream = timeout(config.connect_timeout, P::connect_address(&address))
            .await
            .map_err(|_| EchoError::Timeout(format!("Connection to {address} timed out")))?
            .map_err(Into::into)?;

        Ok(Self {
            stream,
            config,
            last_activity: Instant::now(),
        })
    }

    /// Connects with [`ClientConfig::default`]; see
    /// [`connect_with_config`](Self::connect_with_config).
    pub async fn connect<A: Into<Address>>(address: A) -> Result<Self> {
        Self::connect_with_config(address, ClientConfig::default()).await
    }

    /// Returns `true` if more than `max_idle` has passed since the client
    /// connected or last finished an `echo`.
    pub fn is_idle(&self, max_idle: Duration) -> bool {
        self.last_activity.elapsed() > max_idle
    }

    /// Update the last activity timestamp
    fn update_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Sends `data` and reads the echo back.
    ///
    /// Writing and reading run concurrently, so payloads larger than the
    /// socket buffers cannot deadlock against a server that echoes as it
    /// reads. `write_timeout` bounds the whole write (including the flush);
    /// `read_timeout` bounds each individual read.
    async fn send_and_receive(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.update_activity();

        let write_timeout = self.config.write_timeout;
        let read_timeout = self.config.read_timeout;
        let buffer_size = self.config.buffer_size.max(1);
        let max_response_size = self.config.max_response_size;
        let expected = data.len();

        let response = {
            let (mut reader, mut writer) = tokio::io::split(&mut self.stream);

            let write = async {
                let write_all = async {
                    writer.write_all(data).await?;
                    writer.flush().await
                };
                timeout(write_timeout, write_all)
                    .await
                    .map_err(|_| EchoError::Timeout("Write timeout".to_string()))?
                    .map_err(|e| P::map_io_error(e).into())
            };

            let read = async {
                let mut response = Vec::with_capacity(expected.min(max_response_size));
                let mut buffer = vec![0u8; buffer_size];
                // An echo server returns exactly what was sent, so stop once
                // `expected` bytes have arrived.
                while response.len() < expected {
                    let n = match timeout(read_timeout, reader.read(&mut buffer)).await {
                        Ok(Ok(0)) => break, // Connection closed: return what we have.
                        Ok(Ok(n)) => n,
                        Ok(Err(e)) => return Err(P::map_io_error(e).into()),
                        Err(_) => {
                            return Err(EchoError::Timeout(format!(
                                "Read timeout: expected {expected} bytes, got {} bytes",
                                response.len()
                            )));
                        }
                    };
                    if response.len() + n > max_response_size {
                        return Err(EchoError::Config(format!(
                            "Response too large: {} bytes, max allowed: {max_response_size}",
                            response.len() + n,
                        )));
                    }
                    response.extend_from_slice(&buffer[..n]);
                }
                Ok(response)
            };

            tokio::pin!(write, read);
            let mut written = false;
            loop {
                tokio::select! {
                    result = &mut write, if !written => {
                        result?;
                        written = true;
                    }
                    // The read side finishes after the write side unless the
                    // server closed the connection early or an error occurred;
                    // either way the outcome of the remaining write is moot.
                    result = &mut read => break result?,
                }
            }
        };

        self.update_activity();
        Ok(response)
    }

    /// The client configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Replaces the client configuration (affects later `echo` calls;
    /// `connect_timeout` has no further effect).
    pub fn set_config(&mut self, config: ClientConfig) {
        self.config = config;
    }
}

#[async_trait]
impl<P: StreamProtocol> EchoClient for Client<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
    P::Stream: AsyncRead + AsyncWrite + Unpin,
{
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Validate input size
        if data.len() > self.config.max_response_size {
            return Err(EchoError::Config(format!(
                "Request too large: {} bytes, max allowed: {}",
                data.len(),
                self.config.max_response_size
            )));
        }

        self.send_and_receive(data).await
    }
}

/// Builder for [`ClientConfig`], starting from its defaults.
///
/// # Examples
///
/// ```
/// use echosrv::stream::ClientConfigBuilder;
/// use std::time::Duration;
///
/// let config = ClientConfigBuilder::new()
///     .read_timeout(Duration::from_secs(1))
///     .max_response_size(64 * 1024)
///     .build();
/// assert_eq!(config.read_timeout, Duration::from_secs(1));
/// assert_eq!(config.max_response_size, 64 * 1024);
/// ```
#[derive(Debug, Clone, Default)]
pub struct ClientConfigBuilder {
    config: ClientConfig,
}

impl ClientConfigBuilder {
    /// Starts from [`ClientConfig::default`].
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
        }
    }

    /// Sets [`ClientConfig::read_timeout`].
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.config.read_timeout = timeout;
        self
    }

    /// Sets [`ClientConfig::write_timeout`].
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.config.write_timeout = timeout;
        self
    }

    /// Sets [`ClientConfig::connect_timeout`].
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.connect_timeout = timeout;
        self
    }

    /// Sets [`ClientConfig::buffer_size`].
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    /// Sets [`ClientConfig::max_response_size`].
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.config.max_response_size = size;
        self
    }

    /// Returns the configured [`ClientConfig`].
    pub fn build(self) -> ClientConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::EchoServerTrait;
    use crate::tcp::{TcpConfig, TcpEchoServer, TcpProtocol};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::broadcast;
    use tokio::task::JoinHandle;

    type TcpClient = Client<TcpProtocol>;

    struct Server {
        addr: SocketAddr,
        shutdown: broadcast::Sender<()>,
        handle: JoinHandle<Result<()>>,
    }

    impl Server {
        async fn start() -> Self {
            let server = TcpEchoServer::new(TcpConfig::default().into());
            let shutdown = server.shutdown_signal();
            let bound = server.bind().await.unwrap();
            let addr = *bound.local_addr().as_network().unwrap();
            let handle = tokio::spawn(bound.serve());
            Self {
                addr,
                shutdown,
                handle,
            }
        }

        async fn stop(self) {
            self.shutdown.send(()).unwrap();
            self.handle.await.unwrap().unwrap();
        }
    }

    /// A one-connection server that reads one chunk and replies with `reply`,
    /// then keeps the connection open until the client goes away.
    async fn scripted_server(reply: &'static [u8]) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            if !reply.is_empty() {
                stream.write_all(reply).await.unwrap();
            }
            // Hold the connection open until the client closes it.
            let _ = stream.read(&mut buf).await;
        });
        (addr, handle)
    }

    #[test]
    fn config_defaults() {
        let config = ClientConfig::default();
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.buffer_size, 1024);
        assert_eq!(config.max_response_size, 10 * 1024 * 1024);

        let built = ClientConfigBuilder::default().build();
        assert_eq!(built.read_timeout, config.read_timeout);
        assert_eq!(built.max_response_size, config.max_response_size);
    }

    #[test]
    fn config_builder_sets_every_field() {
        let config = ClientConfigBuilder::new()
            .read_timeout(Duration::from_secs(60))
            .write_timeout(Duration::from_secs(31))
            .connect_timeout(Duration::from_millis(100))
            .buffer_size(2048)
            .max_response_size(1024 * 1024)
            .build();

        assert_eq!(config.read_timeout, Duration::from_secs(60));
        assert_eq!(config.write_timeout, Duration::from_secs(31));
        assert_eq!(config.connect_timeout, Duration::from_millis(100));
        assert_eq!(config.buffer_size, 2048);
        assert_eq!(config.max_response_size, 1024 * 1024);
    }

    #[tokio::test]
    async fn echoes_and_exposes_config() {
        let server = Server::start().await;
        let config = ClientConfigBuilder::new().buffer_size(7).build();
        let mut client = TcpClient::connect_with_config(server.addr, config)
            .await
            .unwrap();
        assert_eq!(client.config().buffer_size, 7);

        // Payload larger than the read buffer is reassembled.
        let payload: Vec<u8> = (0..100u8).collect();
        assert_eq!(client.echo(&payload).await.unwrap(), payload);
        assert_eq!(client.echo(b"").await.unwrap(), b"");

        client.set_config(ClientConfigBuilder::new().buffer_size(3).build());
        assert_eq!(client.config().buffer_size, 3);
        assert_eq!(client.echo_string("hello").await.unwrap(), "hello");

        drop(client);
        server.stop().await;
    }

    #[tokio::test]
    async fn is_idle_tracks_activity() {
        let server = Server::start().await;
        let mut client = TcpClient::connect(server.addr).await.unwrap();

        tokio::time::pause();
        assert!(!client.is_idle(Duration::from_secs(1)));
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(client.is_idle(Duration::from_secs(1)));
        assert!(!client.is_idle(Duration::from_secs(3)));
        // Real I/O below must not race against an auto-advancing clock.
        tokio::time::resume();

        assert_eq!(client.echo(b"ping").await.unwrap(), b"ping");
        assert!(!client.is_idle(Duration::from_secs(1)));

        drop(client);
        server.stop().await;
    }

    #[tokio::test]
    async fn request_larger_than_max_is_rejected_before_sending() {
        let (addr, server) = scripted_server(b"").await;
        let config = ClientConfigBuilder::new().max_response_size(4).build();
        let mut client = TcpClient::connect_with_config(addr, config).await.unwrap();

        match client.echo(b"12345").await {
            Err(EchoError::Config(msg)) => assert!(msg.contains("Request too large"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn response_larger_than_max_is_rejected() {
        let (addr, server) = scripted_server(&[b'x'; 100]).await;
        let config = ClientConfigBuilder::new().max_response_size(16).build();
        let mut client = TcpClient::connect_with_config(addr, config).await.unwrap();

        match client.echo(b"hi").await {
            Err(EchoError::Config(msg)) => assert!(msg.contains("Response too large"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn response_at_max_is_accepted() {
        let server = Server::start().await;
        let config = ClientConfigBuilder::new().max_response_size(5).build();
        let mut client = TcpClient::connect_with_config(server.addr, config)
            .await
            .unwrap();
        assert_eq!(client.echo(b"12345").await.unwrap(), b"12345");
        drop(client);
        server.stop().await;
    }

    #[tokio::test]
    async fn read_timeout_when_server_is_silent() {
        let (addr, server) = scripted_server(b"").await;
        let config = ClientConfigBuilder::new()
            .read_timeout(Duration::from_secs(30))
            .build();
        let mut client = TcpClient::connect_with_config(addr, config).await.unwrap();

        // With the clock paused, the runtime jumps straight to the timeout
        // once nothing else can make progress.
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        match client.echo(b"anyone?").await {
            Err(EchoError::Timeout(msg)) => {
                assert!(msg.contains("expected 7 bytes, got 0"), "{msg}")
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
        assert!(started.elapsed() >= Duration::from_secs(30));
        tokio::time::resume();

        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn read_timeout_after_partial_reply() {
        let (addr, server) = scripted_server(b"ab").await;
        let mut client = TcpClient::connect_with_config(
            addr,
            ClientConfigBuilder::new()
                .read_timeout(Duration::from_millis(200))
                .build(),
        )
        .await
        .unwrap();

        match client.echo(b"abcde").await {
            Err(EchoError::Timeout(msg)) => {
                assert!(msg.contains("expected 5 bytes, got 2"), "{msg}")
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn early_eof_returns_partial_response() {
        // The server replies with fewer bytes and closes: the client returns
        // what it got rather than an error.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 16];
            let _ = stream.read(&mut buf).await.unwrap();
            stream.write_all(b"ab").await.unwrap();
        });
        let mut client = TcpClient::connect(addr).await.unwrap();
        assert_eq!(client.echo(b"abcde").await.unwrap(), b"ab");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_to_closed_port_fails() {
        // Port of a listener that has been closed. (A bound-but-not-listening
        // socket would reserve the port, but macOS silently drops SYNs to it
        // instead of refusing, which turns this into a timeout test.)
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let config = ClientConfigBuilder::new()
            .connect_timeout(Duration::from_secs(5))
            .build();

        match TcpClient::connect_with_config(addr, config).await {
            Err(EchoError::Tcp(e)) => {
                assert_eq!(e.kind(), std::io::ErrorKind::ConnectionRefused)
            }
            Err(other) => panic!("expected Tcp error, got {other:?}"),
            Ok(_) => panic!("connected to a port nobody listens on"),
        }
    }

    #[tokio::test]
    async fn tcp_client_rejects_unix_address() {
        match TcpClient::connect(std::path::PathBuf::from("/tmp/nope.sock")).await {
            Err(EchoError::Tcp(e)) => assert_eq!(e.kind(), std::io::ErrorKind::Unsupported),
            Err(other) => panic!("expected Tcp(Unsupported), got {other:?}"),
            Ok(_) => panic!("TCP client connected to a Unix path"),
        }
    }

    #[tokio::test]
    async fn unix_client_connects_by_path_and_rejects_network_address() {
        use crate::unix::{UnixStreamConfig, UnixStreamEchoServer, UnixStreamProtocol};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.sock");
        let server =
            UnixStreamEchoServer::new(UnixStreamConfig::default().with_socket_path(path.clone()));
        let shutdown = server.shutdown_signal();
        let handle = tokio::spawn(server.bind().await.unwrap().serve());

        let mut client = Client::<UnixStreamProtocol>::connect(Address::Unix(path))
            .await
            .unwrap();
        assert_eq!(client.echo_string("via path").await.unwrap(), "via path");

        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert!(matches!(
            Client::<UnixStreamProtocol>::connect(addr).await,
            Err(EchoError::Unsupported(_))
        ));

        drop(client);
        shutdown.send(()).unwrap();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn echo_string_rejects_invalid_utf8_reply() {
        let (addr, server) = scripted_server(&[0xff, 0xfe]).await;
        let mut client = TcpClient::connect(addr).await.unwrap();
        assert!(matches!(
            client.echo_string("ok").await,
            Err(EchoError::Utf8(_))
        ));
        drop(client);
        server.await.unwrap();
    }
}
