//! The [`EchoServerTrait`] and [`EchoClient`] traits.

use crate::{EchoError, Result};
use async_trait::async_trait;

/// Interface implemented by every echo server in this crate.
///
/// Servers also have an inherent `bind()` method that returns a bound server
/// exposing the actual local address (useful with port `0`); `run()` is
/// `bind()` followed by `serve()`.
#[async_trait]
pub trait EchoServerTrait {
    /// Binds the server socket and serves until a shutdown is requested.
    ///
    /// Returns `Ok(())` after a graceful shutdown. Signal handling (Ctrl-C,
    /// SIGTERM) is the caller's responsibility: forward it via
    /// [`shutdown_signal`](Self::shutdown_signal).
    async fn run(&self) -> Result<()>;

    /// Returns a sender that gracefully shuts the server down when `send(())` is called.
    ///
    /// A shutdown requested before `run()` starts is honored: `run()` then
    /// returns immediately after binding. On shutdown the server stops
    /// accepting, cancels in-flight connection tasks and waits for them to finish.
    fn shutdown_signal(&self) -> tokio::sync::broadcast::Sender<()>;
}

/// Interface implemented by every echo client in this crate.
#[async_trait]
pub trait EchoClient {
    /// Sends `data` to the echo server and returns the echoed response.
    ///
    /// What counts as "the response" depends on the protocol: stream clients
    /// read until `data.len()` bytes arrived (or the server closed the
    /// connection), datagram clients return one reply datagram, and the HTTP
    /// client returns the response body.
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>>;

    /// Sends a string and returns the echoed string.
    ///
    /// Fails with [`EchoError::Utf8`] if the response is not valid UTF-8.
    async fn echo_string(&mut self, data: &str) -> Result<String> {
        let response = self.echo(data.as_bytes()).await?;
        String::from_utf8(response).map_err(EchoError::Utf8)
    }
}
