//! Datagram-based echo server and client functionality
//!
//! This module provides generic datagram-based echo servers and clients
//! that can work with any datagram protocol (UDP, Unix datagrams, etc.).

pub mod config;
pub mod protocol;
pub mod server;
pub mod client;

pub use config::DatagramConfig;
pub use protocol::DatagramProtocol;
pub use server::DatagramEchoServer;
pub use client::DatagramEchoClient; 