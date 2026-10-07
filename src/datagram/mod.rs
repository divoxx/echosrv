//! Datagram-based echo server and client functionality
//!
//! This module provides generic datagram-based echo servers and clients
//! that can work with any datagram protocol (UDP, Unix datagrams, etc.).

pub mod client;
pub mod config;
pub mod protocol;
pub mod server;

pub use client::DatagramEchoClient;
pub use config::{DEFAULT_DATAGRAM_BUFFER_SIZE, DatagramClientConfig, DatagramConfig};
pub use protocol::DatagramProtocol;
pub use server::{BoundDatagramServer, DatagramEchoServer};
