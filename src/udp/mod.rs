//! UDP echo server and client.
//!
//! [`UdpEchoServer`] and [`UdpEchoClient`] are type aliases for the generic
//! [`DatagramEchoServer`](crate::datagram::DatagramEchoServer) and
//! [`DatagramEchoClient`](crate::datagram::DatagramEchoClient) over
//! [`UdpProtocol`]. Build the server from a [`UdpConfig`] converted into a
//! [`DatagramConfig`](crate::datagram::DatagramConfig):
//!
//! ```
//! use echosrv::udp::{UdpConfig, UdpEchoServer};
//!
//! let server = UdpEchoServer::new(UdpConfig::default().into());
//! ```

pub mod config;
pub mod datagram_protocol;
pub mod server;
pub mod socket_builder;
#[cfg(test)]
mod tests;

pub use config::UdpConfig;
pub use datagram_protocol::UdpProtocol;
pub use server::UdpEchoServer;

/// UDP echo client: the generic
/// [`DatagramEchoClient`](crate::datagram::DatagramEchoClient) over
/// [`UdpProtocol`].
///
/// Each [`echo`](crate::EchoClient::echo) sends one datagram and waits for one
/// reply from the server's address.
pub type UdpEchoClient = crate::datagram::DatagramEchoClient<UdpProtocol>;
