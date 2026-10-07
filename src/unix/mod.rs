//! Unix domain socket echo servers and clients (stream and datagram).
//!
//! * [`UnixStreamEchoServer`] / [`UnixStreamEchoClient`] / [`UnixStreamConfig`]
//!   behave like their TCP counterparts, addressed by a socket path.
//! * [`UnixDatagramEchoServer`] / [`UnixDatagramEchoClient`] /
//!   [`UnixDatagramConfig`] echo each datagram to the sender's socket path.
//!   Senders must be bound to a path to receive a reply;
//!   [`UnixDatagramEchoClient`] binds a temporary one for that.
//!
//! Socket files: when binding, a stale socket file (nothing listening on it)
//! is removed and the path re-bound, while a live socket or a non-socket file
//! is an error; missing parent directories are created. A socket file the
//! server created is removed when the server stops. Socket files of inherited
//! descriptors are never removed.
//!
//! # Examples
//!
//! ```
//! use echosrv::unix::{
//!     UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixStreamConfig,
//!     UnixStreamEchoClient, UnixStreamEchoServer,
//! };
//! use echosrv::{EchoClient, EchoServerTrait};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = tempfile::tempdir()?;
//!
//! // Stream
//! let path = dir.path().join("stream.sock");
//! let config = UnixStreamConfig::default().with_socket_path(path.clone());
//! let server = UnixStreamEchoServer::new(config);
//! let shutdown = server.shutdown_signal();
//! let handle = tokio::spawn(server.bind().await?.serve());
//! let mut client = UnixStreamEchoClient::connect(path.clone()).await?;
//! assert_eq!(client.echo_string("hello").await?, "hello");
//! shutdown.send(())?;
//! handle.await??;
//! assert!(!path.exists()); // the server removed its socket file
//!
//! // Datagram
//! let path = dir.path().join("dgram.sock");
//! let config = UnixDatagramConfig::default().with_socket_path(path.clone());
//! let server = UnixDatagramEchoServer::new(config);
//! let shutdown = server.shutdown_signal();
//! let handle = tokio::spawn(server.bind().await?.serve());
//! let mut client = UnixDatagramEchoClient::connect(path).await?;
//! assert_eq!(client.echo(b"hello").await?, b"hello");
//! shutdown.send(())?;
//! handle.await??;
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod config;
pub mod datagram_protocol;
pub mod server;
mod socket_file;
pub mod stream_protocol;

#[cfg(test)]
mod tests;

pub use config::{UnixDatagramConfig, UnixStreamConfig};

pub use client::{UnixDatagramEchoClient, UnixStreamEchoClient};
pub use server::{UnixDatagramEchoServer, UnixStreamEchoServer};

pub use datagram_protocol::{ManagedUnixDatagram, UnixDatagramExt, UnixDatagramProtocol};
pub use stream_protocol::{ManagedUnixListener, UnixStreamExt, UnixStreamProtocol};
