use crate::common::StreamEchoServer;
use super::stream_protocol::TcpProtocol;

/// TCP echo server that handles TCP connections
///
/// This is a type alias for `StreamEchoServer<TcpProtocol>`.
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
///     };
///
///     let server = TcpEchoServer::new(config);
///     server.run().await?;
///     Ok(())
/// }
/// ```
///
/// Server with graceful shutdown:
///
/// ```no_run
/// use echosrv::tcp::{TcpConfig, TcpEchoServer};
/// use echosrv::common::EchoServerTrait;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = TcpConfig::default();
///     let server = TcpEchoServer::new(config);
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
pub type TcpEchoServer = StreamEchoServer<TcpProtocol>;

 