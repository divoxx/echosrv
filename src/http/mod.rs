//! HTTP echo server implementation
//!
//! This module provides HTTP echo server functionality that echoes HTTP requests
//! back as HTTP responses, preserving headers and request body.

pub mod config;
pub mod protocol;

#[cfg(test)]
mod tests;

pub use config::HttpConfig;
pub use protocol::HttpProtocol;
pub use crate::stream::{StreamEchoServer, StreamEchoClient};

/// Type alias for HTTP echo server
pub type HttpEchoServer = StreamEchoServer<HttpProtocol>;
/// Type alias for HTTP echo client
pub type HttpEchoClient = StreamEchoClient<HttpProtocol>; 