use crate::common::DatagramEchoServer;
use super::datagram_protocol::UdpProtocol;

/// UDP echo server that handles UDP datagrams
///
/// This is a type alias for `DatagramEchoServer<UdpProtocol>`.
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
///     };
///
///     let server = UdpEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Server with graceful shutdown:
///
/// ```no_run
/// use echosrv::udp::{UdpConfig, UdpEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = UdpConfig::default();
///     let server = UdpEchoServer::new(config);
///     let shutdown_signal = server.shutdown_signal();
///
///     // Run server in background
///     let server_handle = tokio::spawn(async move {
///         server.run().await
///     });
///
///     // Do other work...
///     
///     // Gracefully shutdown
///     let _ = shutdown_signal.send(());
///     server_handle.await??;
///     Ok(())
/// }
/// ```
pub type UdpEchoServer = DatagramEchoServer<UdpProtocol>;

 