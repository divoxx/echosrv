//! Generic datagram echo server and client.
//!
//! [`DatagramEchoServer`] is generic over a [`DatagramProtocol`]; UDP
//! ([`crate::udp`]) and Unix datagram sockets ([`crate::unix`]) are the
//! implementations shipped with the crate. Each received datagram is sent back
//! to its sender. [`DatagramEchoClient`] works with protocols addressed by
//! [`SocketAddr`](std::net::SocketAddr) (i.e. UDP).

pub mod client;
pub mod config;
pub mod protocol;
pub mod server;

pub use client::DatagramEchoClient;
pub use config::{DEFAULT_DATAGRAM_BUFFER_SIZE, DatagramClientConfig, DatagramConfig};
pub use protocol::DatagramProtocol;
pub use server::{BoundDatagramServer, DatagramEchoServer};
