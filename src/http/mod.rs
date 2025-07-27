//! HTTP echo server implementation
//!
//! This module provides HTTP echo server functionality that echoes HTTP requests
//! back as HTTP responses, preserving headers and request body.

pub mod config;
pub mod protocol;

#[cfg(test)]
mod tests;

pub use crate::stream::{Client as StreamClient, StreamEchoServer};
pub use config::HttpConfig;
pub use protocol::HttpProtocol;

/// Type alias for HTTP echo server
pub type HttpEchoServer = StreamEchoServer<HttpProtocol>;
/// Type alias for HTTP echo client
pub type HttpEchoClient = StreamClient<HttpProtocol>;
