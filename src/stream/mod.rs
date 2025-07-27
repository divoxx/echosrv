//! Stream-based echo server and client functionality
//!
//! This module provides generic stream-based echo servers and clients
//! that can work with any stream protocol (TCP, Unix streams, etc.).

pub mod config;
pub mod protocol;
pub mod server;
pub mod client;

pub use config::StreamConfig;
pub use protocol::StreamProtocol;
pub use server::StreamEchoServer;
pub use client::{Client, ClientConfig, ClientConfigBuilder}; 