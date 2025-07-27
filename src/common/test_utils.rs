use crate::common::EchoServerTrait;
use crate::{EchoError, Result};
use std::net::SocketAddr;
use tokio::task::JoinHandle;

/// Creates a controlled test server with connection limit for integration tests
///
/// This function creates a TCP server with a specific connection limit
/// and returns both the server handle and the address it's bound to.
pub async fn create_controlled_test_server_with_limit(
    max_connections: usize,
) -> Result<(JoinHandle<Result<()>>, SocketAddr)> {
    use crate::{TcpConfig, TcpEchoServer};
    use std::time::Duration;
    use tokio::net::TcpListener;

    // First bind to get the actual address
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| EchoError::Config(format!("Failed to bind listener: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| EchoError::Config(format!("Failed to get local address: {e}")))?;
    drop(listener); // Close the listener so the server can bind to the same address

    let config = TcpConfig {
        bind_addr: addr,
        max_connections,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = TcpEchoServer::new(config.into());

    let server_handle = tokio::spawn(async move { server.run().await });

    Ok((server_handle, addr))
}
