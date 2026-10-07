//! Generic stream echo server and client.
//!
//! [`StreamEchoServer`] is generic over a [`StreamProtocol`]; TCP
//! ([`crate::tcp`]), HTTP ([`crate::http`]) and Unix stream sockets
//! ([`crate::unix`]) are the implementations shipped with the crate. The server
//! echoes every byte it reads on a connection until the client closes it or
//! the read timeout expires. [`Client`] is the matching generic client.

pub mod client;
pub mod config;
pub mod protocol;
pub mod server;

pub use client::{Client, ClientConfig, ClientConfigBuilder};
pub use config::StreamConfig;
pub use protocol::StreamProtocol;
pub use server::{BoundStreamServer, StreamEchoServer};
