//! The UDP echo server type.

use super::datagram_protocol::UdpProtocol;
use crate::datagram::DatagramEchoServer;

/// UDP echo server: sends every datagram back to its sender.
///
/// A type alias for [`DatagramEchoServer`]`<`[`UdpProtocol`]`>`; see there for
/// the full behavior. `new` takes a [`DatagramConfig`](crate::datagram::DatagramConfig), so
/// convert a [`UdpConfig`](super::UdpConfig) with `.into()`.
///
/// # Examples
///
/// Basic server setup and running:
///
/// ```no_run
/// use echosrv::udp::{UdpConfig, UdpEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = UdpConfig {
///         bind_addr: "127.0.0.1:8080".parse()?,
///         buffer_size: 1024,
///         read_timeout: Duration::from_secs(30),
///         write_timeout: Duration::from_secs(30),
///         ..Default::default()
///     };
///
///     let server = UdpEchoServer::new(config.into());
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Running on an ephemeral port and shutting down gracefully:
///
/// ```
/// use echosrv::{EchoClient, EchoServerTrait};
/// use echosrv::udp::{UdpConfig, UdpEchoClient, UdpEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// let server = UdpEchoServer::new(UdpConfig::default().into()); // 127.0.0.1:0
/// let shutdown = server.shutdown_signal();
/// let bound = server.bind().await?;
/// let addr = *bound.local_addr().as_network().unwrap();
/// let handle = tokio::spawn(bound.serve());
///
/// let mut client = UdpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo(b"hello").await?, b"hello");
///
/// shutdown.send(()).unwrap();
/// handle.await.unwrap()?;
/// # Ok(())
/// # }
/// ```
pub type UdpEchoServer = DatagramEchoServer<UdpProtocol>;
