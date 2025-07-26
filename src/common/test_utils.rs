use crate::{Result, EchoError};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tracing::{error, info};

/// Helper function to create a simple test server that can be controlled and enforces connection limits
///
/// This function creates a TCP test server that can be used for testing both TCP and UDP clients.
/// It enforces connection limits and provides detailed logging for debugging.
///
/// # Arguments
///
/// * `max_connections` - Maximum number of concurrent connections to allow
///
/// # Returns
///
/// A tuple containing:
/// * A join handle for the server task
/// * The address the server is bound to
///
/// # Examples
///
/// ```no_run
/// use echosrv::common::create_controlled_test_server_with_limit;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let (server_handle, addr) = create_controlled_test_server_with_limit(10).await?;
///     
///     // Use the server for testing...
///     
///     // Clean shutdown
///     server_handle.abort();
///     Ok(())
/// }
/// ```
pub async fn create_controlled_test_server_with_limit(max_connections: usize) -> Result<(tokio::task::JoinHandle<Result<()>>, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(EchoError::Tcp)?;
    let addr = listener.local_addr().map_err(EchoError::Tcp)?;
    let connection_count = Arc::new(AtomicUsize::new(0));

    let server_handle = {
        let connection_count = connection_count.clone();
        tokio::spawn(async move {
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(5),
                    listener.accept()
                ).await {
                    Ok(Ok((socket, addr))) => {
                        let current_count = connection_count.load(Ordering::SeqCst);
                        if current_count >= max_connections {
                            // Exceeded connection limit, drop connection
                            info!("Test server: Connection from {} rejected (limit reached)", addr);
                            drop(socket);
                            continue;
                        }
                        
                        connection_count.fetch_add(1, Ordering::SeqCst);
                        let new_count = connection_count.load(Ordering::SeqCst);
                        info!("Test server: New connection from {} (total: {})", addr, new_count);
                        
                        let connection_count = connection_count.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_test_connection(socket, addr).await {
                                error!("Test server: Error handling connection from {}: {}", addr, e);
                            }
                            let final_count = connection_count.fetch_sub(1, Ordering::SeqCst) - 1;
                            info!("Test server: Connection from {} closed (total: {})", addr, final_count);
                        });
                    }
                    Ok(Err(e)) => {
                        error!("Test server: Failed to accept connection: {}", e);
                        break;
                    }
                    Err(_) => {
                        // Timeout - server is done
                        break;
                    }
                }
            }
            info!("Test server stopped");
            Ok(())
        })
    };
    Ok((server_handle, addr))
}

/// Simple connection handler for tests
///
/// This function handles a single TCP connection in test scenarios,
/// echoing back any data received from the client.
///
/// # Arguments
///
/// * `socket` - The TCP stream to handle
/// * `addr` - The address of the client
///
/// # Examples
///
/// ```no_run
/// use echosrv::common::handle_test_connection;
/// use tokio::net::TcpStream;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let socket = TcpStream::connect("127.0.0.1:8080").await?;
///     let addr = socket.peer_addr()?;
///     handle_test_connection(socket, addr).await?;
///     Ok(())
/// }
/// ```
pub async fn handle_test_connection(
    mut socket: tokio::net::TcpStream,
    addr: SocketAddr,
) -> Result<()> {
    let mut buffer = [0; 1024];

    loop {
        let n = socket
            .read(&mut buffer)
            .await
            .map_err(EchoError::Tcp)?;

        if n == 0 {
            // Connection closed by client
            break;
        }

        // Echo back the received data
        socket
            .write_all(&buffer[..n])
            .await
            .map_err(EchoError::Tcp)?;

        socket
            .flush()
            .await
            .map_err(EchoError::Tcp)?;

        info!("Test server: Echoed {} bytes to {}", n, addr);
    }

    Ok(())
}

/// Helper function to create a simple test server without connection limits
///
/// This function creates a basic TCP test server that accepts all connections
/// and echoes back any data received.
///
/// # Returns
///
/// A tuple containing:
/// * A join handle for the server task
/// * The address the server is bound to
///
/// # Examples
///
/// ```no_run
/// use echosrv::common::create_test_server;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let (server_handle, addr) = create_test_server().await?;
///     
///     // Use the server for testing...
///     
///     // Clean shutdown
///     server_handle.abort();
///     Ok(())
/// }
/// ```
pub async fn create_test_server() -> Result<(tokio::task::JoinHandle<Result<()>>, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(EchoError::Tcp)?;
    let addr = listener.local_addr().map_err(EchoError::Tcp)?;

    let server_handle = tokio::spawn(async move {
        loop {
            match tokio::time::timeout(
                Duration::from_secs(5),
                listener.accept()
            ).await {
                Ok(Ok((socket, addr))) => {
                    tokio::spawn(async move {
                        if let Err(e) = handle_test_connection(socket, addr).await {
                            error!("Test server: Error handling connection from {}: {}", addr, e);
                        }
                    });
                }
                Ok(Err(e)) => {
                    error!("Test server: Failed to accept connection: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout - server is done
                    break;
                }
            }
        }
        info!("Test server stopped");
        Ok(())
    });
    Ok((server_handle, addr))
} 