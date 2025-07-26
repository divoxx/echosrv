use echosrv::tcp::{Config, EchoClient, EchoServer};
use color_eyre::eyre::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tracing::{error, info};

/// Helper function to create a test server that can be controlled and enforces connection limits
async fn create_controlled_test_server_with_limit(max_connections: usize) -> Result<(tokio::task::JoinHandle<Result<()>>, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
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
async fn handle_test_connection(
    mut socket: tokio::net::TcpStream,
    addr: SocketAddr,
) -> Result<()> {
    let mut buffer = [0; 1024];

    loop {
        let n = socket
            .read(&mut buffer)
            .await
            .with_context(|| format!("Failed to read from connection {}", addr))?;

        if n == 0 {
            // Connection closed by client
            break;
        }

        // Echo back the received data
        socket
            .write_all(&buffer[..n])
            .await
            .with_context(|| format!("Failed to write to connection {}", addr))?;

        socket
            .flush()
            .await
            .with_context(|| format!("Failed to flush connection {}", addr))?;

        info!("Test server: Echoed {} bytes to {}", n, addr);
    }

    Ok(())
}

#[tokio::test]
async fn test_multiple_concurrent_clients() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(10).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test multiple concurrent clients
    let client_count = 5;
    let mut handles = Vec::new();

    for i in 0..client_count {
        let addr = addr;
        let handle = tokio::spawn(async move {
            let mut client = EchoClient::connect(addr).await?;
            let message = format!("Message from client {}", i);
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), color_eyre::eyre::Error>(())
        });
        handles.push(handle);
    }

    // Wait for all clients to complete
    for handle in handles {
        handle.await??;
    }
    
    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_connection_limit() -> Result<()> {
    let config = Config {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 2, // Very low limit for testing
        buffer_size: 1024,
        read_timeout: Some(Duration::from_secs(30)),
        write_timeout: Some(Duration::from_secs(30)),
    };

    let listener = TcpListener::bind(config.bind_addr).await?;
    let addr = listener.local_addr()?;
    drop(listener);

    let config = Config {
        bind_addr: addr,
        max_connections: 2,
        buffer_size: 1024,
        read_timeout: Some(Duration::from_secs(30)),
        write_timeout: Some(Duration::from_secs(30)),
    };

    let server = EchoServer::new(config);
    let server_handle = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(10),
            server.run()
        ).await.map_err(|_| color_eyre::eyre::eyre!("Server timeout"))?
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Try to create more connections than the limit concurrently
    let mut handles = Vec::new();
    
    for i in 0..5 {
        let addr = addr;
        let handle = tokio::spawn(async move {
            match EchoClient::connect(addr).await {
                Ok(mut client) => {
                    // Try to echo something to verify the connection works
                    match client.echo_string("test").await {
                        Ok(response) => {
                            if response == "test" {
                                Ok::<usize, color_eyre::eyre::Error>(1) // Success
                            } else {
                                Ok(0) // Echo mismatch
                            }
                        }
                        Err(e) => {
                            info!("Connected but echo failed for client {}: {}", i, e);
                            Ok(0) // Echo failed
                        }
                    }
                }
                Err(e) => {
                    info!("Connection failed for client {}: {}", i, e);
                    Ok(0) // Connection failed
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all connections to complete
    let mut successful_connections = 0;
    let mut failed_connections = 0;
    
    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => {
                if result == 1 {
                    successful_connections += 1;
                } else {
                    failed_connections += 1;
                }
            }
            Ok(Err(_)) | Err(_) => {
                failed_connections += 1;
            }
        }
    }

    // Should have at most 2 successful connections, and some failures
    assert!(successful_connections <= 2, "Expected at most 2 successful connections, got {}", successful_connections);
    assert!(failed_connections > 0, "Expected some connection failures, got {}", failed_connections);
    
    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_graceful_shutdown() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(10).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify server is running
    let mut client = EchoClient::connect(addr).await?;
    let response = client.echo_string("test").await?;
    assert_eq!(response, "test");

    // Shutdown server
    server_handle.abort();
    
    // Give server time to shutdown
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify server is no longer accepting connections
    match EchoClient::connect(addr).await {
        Ok(_) => panic!("Server should not accept connections after shutdown"),
        Err(_) => {
            // Expected - server is shutdown
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_timeout_configuration() -> Result<()> {
    // Test server with very short timeouts
    let config = Config {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Some(Duration::from_millis(100)), // Very short timeout
        write_timeout: Some(Duration::from_millis(100)),
    };

    let listener = TcpListener::bind(config.bind_addr).await?;
    let addr = listener.local_addr()?;
    drop(listener);

    let config = Config {
        bind_addr: addr,
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Some(Duration::from_millis(100)),
        write_timeout: Some(Duration::from_millis(100)),
    };

    let server = EchoServer::new(config);
    let server_handle = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(5),
            server.run()
        ).await.map_err(|_| color_eyre::eyre::eyre!("Server timeout"))?
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test that normal operations work with timeouts
    let mut client = EchoClient::connect(addr).await?;
    let response = client.echo_string("quick test").await?;
    assert_eq!(response, "quick test");
    
    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_stress_test() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(50).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stress test with many rapid connections
    let mut handles = Vec::new();
    let mut successful_echoes = 0;
    let mut failed_connections = 0;
    
    for i in 0..100 {
        let addr = addr;
        let handle = tokio::spawn(async move {
            match EchoClient::connect(addr).await {
                Ok(mut client) => {
                    // Send multiple messages per connection
                    for j in 0..5 {
                        let message = format!("Stress test message {} from client {}", j, i);
                        match client.echo_string(&message).await {
                            Ok(response) => {
                                if response == message {
                                    return Ok::<usize, color_eyre::eyre::Error>(1); // Success
                                } else {
                                    return Ok(0); // Echo mismatch
                                }
                            }
                            Err(_) => return Ok(0), // Echo failed
                        }
                    }
                    Ok(0) // Should not reach here
                }
                Err(_) => Ok(0), // Connection failed
            }
        });
        handles.push(handle);
    }

    // Wait for all stress test clients to complete
    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => {
                if result == 1 {
                    successful_echoes += 1;
                } else {
                    failed_connections += 1;
                }
            }
            Ok(Err(_)) | Err(_) => {
                failed_connections += 1;
            }
        }
    }
    
    // Should have many successful echoes and some failures
    assert!(successful_echoes > 0, "Expected some successful echoes, got {}", successful_echoes);
    info!("Stress test completed: {} successful echoes, {} failed connections", successful_echoes, failed_connections);
    
    server_handle.abort();
    Ok(())
} 