//! Unix Domain Socket implementations for the echo server
//!
//! This module provides both stream-based and datagram-based Unix domain socket
//! echo servers and clients. Unix domain sockets provide efficient inter-process
//! communication on Unix-like systems.
//!
//! # Examples
//!
//! ## Unix Stream Server
//!
//! ```no_run
//! use echosrv::unix::{UnixStreamConfig, UnixStreamEchoServer};
//! use echosrv::common::EchoServerTrait;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = UnixStreamConfig {
//!         max_connections: 100,
//!         buffer_size: 1024,
//!         read_timeout: Duration::from_secs(30),
//!         write_timeout: Duration::from_secs(30),
//!         ..UnixStreamConfig::default().with_socket_path("/tmp/echo.sock".into())
//!     };
//!
//!     let server = UnixStreamEchoServer::new(config);
//!     server.run().await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Unix Datagram Server
//!
//! ```no_run
//! use echosrv::unix::{UnixDatagramConfig, UnixDatagramEchoServer};
//! use echosrv::common::EchoServerTrait;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = UnixDatagramConfig {
//!         buffer_size: 1024,
//!         read_timeout: Duration::from_secs(30),
//!         write_timeout: Duration::from_secs(30),
//!         ..UnixDatagramConfig::default().with_socket_path("/tmp/echo_dgram.sock".into())
//!     };
//!
//!     let server = UnixDatagramEchoServer::new(config);
//!     server.run().await?;
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod config;
pub mod datagram_protocol;
pub mod server;
mod socket_file;
pub mod stream_protocol;

#[cfg(test)]
mod tests;

// Re-export configuration types
pub use config::{UnixDatagramConfig, UnixStreamConfig};

// Re-export server and client types
pub use client::{UnixDatagramEchoClient, UnixStreamEchoClient};
pub use server::{UnixDatagramEchoServer, UnixStreamEchoServer};

// Re-export protocol implementations
pub use datagram_protocol::{ManagedUnixDatagram, UnixDatagramExt, UnixDatagramProtocol};
pub use stream_protocol::{ManagedUnixListener, UnixStreamExt, UnixStreamProtocol};
