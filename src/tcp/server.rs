//! The TCP echo server type.

use super::stream_protocol::TcpProtocol;
use crate::stream::StreamEchoServer;

/// TCP echo server: echoes every byte received on each connection.
///
/// A type alias for [`StreamEchoServer`]`<`[`TcpProtocol`]`>`; see there for
/// the full behavior. `new` takes a [`StreamConfig`](crate::stream::StreamConfig), so
/// convert a [`TcpConfig`](super::TcpConfig) with `.into()`.
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::tcp::{TcpConfig, TcpEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = TcpConfig {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         max_connections: 100,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///         ..Default::default()
///     };
///
///     let server = TcpEchoServer::new(config.into());
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Running on an ephemeral port and shutting down gracefully:
///
/// ```
/// use echosrv::{EchoClient, EchoServerTrait};
/// use echosrv::tcp::{TcpConfig, TcpEchoClient, TcpEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// let server = TcpEchoServer::new(TcpConfig::default().into()); // 127.0.0.1:0
/// let shutdown = server.shutdown_signal();
/// let bound = server.bind().await?;
/// let addr = *bound.local_addr().as_network().unwrap();
/// let handle = tokio::spawn(bound.serve());
///
/// let mut client = TcpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo(b"hello").await?, b"hello");
///
/// shutdown.send(()).unwrap();
/// handle.await.unwrap()?;
/// # Ok(())
/// # }
/// ```
pub type TcpEchoServer = StreamEchoServer<TcpProtocol>;
