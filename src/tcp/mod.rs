//! TCP echo server and client.
//!
//! [`TcpEchoServer`] and [`TcpEchoClient`] are type aliases for the generic
//! [`StreamEchoServer`](crate::stream::StreamEchoServer) and
//! [`Client`](crate::stream::Client) over [`TcpProtocol`]. Build the server from
//! a [`TcpConfig`] converted into a [`StreamConfig`](crate::stream::StreamConfig):
//!
//! ```
//! use echosrv::tcp::{TcpConfig, TcpEchoServer};
//!
//! let server = TcpEchoServer::new(TcpConfig::default().into());
//! ```

pub mod config;
pub mod server;
pub mod socket_builder;
pub mod stream_protocol;
#[cfg(test)]
mod tests;

pub use config::TcpConfig;
pub use server::TcpEchoServer;
pub use stream_protocol::TcpProtocol;

/// TCP echo client: the generic stream [`Client`](crate::stream::Client) over
/// [`TcpProtocol`].
///
/// [`echo`](crate::EchoClient::echo) writes the payload and reads until the
/// same number of bytes came back. Timeouts and the response size limit are
/// set with [`ClientConfig`](crate::stream::ClientConfig).
pub type TcpEchoClient = crate::stream::Client<TcpProtocol>;
