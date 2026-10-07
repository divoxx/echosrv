use crate::{EchoError, Result};
use async_trait::async_trait;

/// Common trait for echo servers
///
/// This trait defines the common interface that all echo servers
/// (TCP, UDP, etc.) must implement.
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

/// Common trait for echo clients
///
/// This trait defines the common interface that all echo clients
/// (TCP, UDP, etc.) must implement.
#[async_trait]
pub trait EchoClient {
    /// Sends data to the echo server and returns the echoed response
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>>;

    /// Sends a string and returns the echoed string
    async fn echo_string(&mut self, data: &str) -> Result<String> {
        let response = self.echo(data.as_bytes()).await?;
        String::from_utf8(response).map_err(EchoError::Utf8)
    }
}
