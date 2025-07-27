use echosrv::network::Address;
use echosrv::performance::{BufferPool, global_pool};
use echosrv::security::{ConnectionTracker, RateLimiter, ResourceLimits, SizeValidator};
use echosrv::{EchoClient, EchoError, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::time::Duration;
use tempfile::tempdir;

/// Integration test demonstrating the new address system
#[tokio::test]
async fn test_address_system() {
    // Test network address creation
    let net_addr: Address = "127.0.0.1:8080".into();
    assert!(net_addr.is_network());
    assert!(!net_addr.is_unix());

    // Test Unix address creation
    let unix_addr: Address = "unix:/tmp/test.sock".into();
    assert!(!unix_addr.is_network());
    assert!(unix_addr.is_unix());

    // Test display formatting
    assert_eq!(net_addr.to_string(), "127.0.0.1:8080");
    assert_eq!(unix_addr.to_string(), "unix:/tmp/test.sock");
}

/// Integration test for new configuration system
#[tokio::test]
async fn test_unified_config() {
    // Test address system
    let net_addr: Address = "127.0.0.1:8080".into();
    assert!(net_addr.is_network());

    let unix_addr: Address = "unix:/tmp/test.sock".into();
    assert!(unix_addr.is_unix());

    // For now, just test that the address system works
    // The full config system would be tested when fully integrated
}

/// Integration test for security and rate limiting
#[tokio::test]
async fn test_security_features() {
    // Test rate limiter - basic functionality
    let _limiter = RateLimiter::new(2); // 2 requests per second

    // For now, just test that we can create it
    // The full rate limiting would need more complex testing with timing

    // Test connection tracker
    let limits = ResourceLimits {
        max_concurrent_connections: 3,
        ..Default::default()
    };
    let tracker = ConnectionTracker::new(limits);

    // Acquire 3 connections
    let _guard1 = tracker.acquire_connection().await.unwrap();
    let _guard2 = tracker.acquire_connection().await.unwrap();
    let _guard3 = tracker.acquire_connection().await.unwrap();

    // 4th connection should timeout
    assert!(tracker.acquire_connection().await.is_err());

    // Test size validator
    let validator = SizeValidator::new(100);
    assert!(validator.validate_size(50).is_ok());
    assert!(validator.validate_size(150).is_err());
}

/// Integration test for performance optimizations
#[tokio::test]
async fn test_performance_optimizations() {
    // Test buffer pool
    let pool = BufferPool::new(1024, 5);

    // Get a buffer, use it, and return it
    {
        let mut buffer = pool.get();
        buffer.extend_from_slice(b"test data");
        assert_eq!(buffer.len(), 9);
    } // Buffer returns to pool here

    // Get another buffer - should be reused
    let buffer = pool.get();
    assert!(buffer.is_empty()); // Should be cleared
    assert_eq!(buffer.capacity(), 1024);

    // Test global pool
    let global_buffer = global_pool().get();
    assert!(global_buffer.capacity() > 0);
}

/// Integration test for client with timeout handling
#[tokio::test]
async fn test_client() {
    use echosrv::stream::ClientConfigBuilder;

    // Test client config builder
    let config = ClientConfigBuilder::new()
        .read_timeout(Duration::from_secs(60))
        .write_timeout(Duration::from_secs(30))
        .max_response_size(1024 * 1024)
        .build();

    assert_eq!(config.read_timeout, Duration::from_secs(60));
    assert_eq!(config.write_timeout, Duration::from_secs(30));
    assert_eq!(config.max_response_size, 1024 * 1024);

    // Test idle detection
    // Note: This would require a real connection to test fully
    // For now, just verify the config works
    assert!(config.read_timeout > Duration::from_secs(0));
}

/// Comprehensive end-to-end test with all improvements
#[tokio::test]
async fn test_end_to_end_improvements() -> Result<(), Box<dyn std::error::Error>> {
    use tokio::net::TcpListener;

    // First bind to get the actual address
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener); // Close the listener so the server can bind to the same address

    // Create server with configuration
    let config = TcpConfig {
        bind_addr: addr,
        max_connections: 50,
        buffer_size: 2048,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = TcpEchoServer::new(config.clone().into());

    // Start server
    let server_handle = tokio::spawn(async move { server.run().await });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test with multiple clients using different data sizes
    let test_cases = vec![
        b"small".to_vec(),
        vec![b'x'; 1024],                                 // 1KB
        vec![b'y'; 4096],                                 // 4KB
        (0..255).cycle().take(8192).collect::<Vec<u8>>(), // 8KB with patterns
    ];

    for (i, data) in test_cases.iter().enumerate() {
        let mut client = TcpEchoClient::connect(addr).await?;
        let response = client.echo(data).await?;
        assert_eq!(response, *data, "Test case {i} failed");
    }

    // Test concurrent access
    let mut handles = Vec::new();
    for i in 0..10 {
        let addr = addr;
        let handle = tokio::spawn(async move {
            let mut client = TcpEchoClient::connect(addr).await?;
            let message = format!("Concurrent test message {i}");
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), EchoError>(())
        });
        handles.push(handle);
    }

    // Wait for all concurrent tests to complete
    for handle in handles {
        handle.await??;
    }

    // Test binary data with null bytes
    let binary_data = vec![0, 1, 2, 0, 255, 128, 0, 3];
    let mut client = TcpEchoClient::connect(addr).await?;
    let response = client.echo(&binary_data).await?;
    assert_eq!(response, binary_data);

    // Clean shutdown
    server_handle.abort();

    Ok(())
}

/// Test error handling improvements
#[tokio::test]
async fn test_error_handling() {
    // Test connection to non-existent server
    let result =
        TcpEchoClient::connect("127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap()).await;
    assert!(result.is_err());

    // Verify error types are meaningful
    match result {
        Err(EchoError::Config(_)) | Err(EchoError::Tcp(_)) => {
            // Expected error types
        }
        Err(other) => panic!("Unexpected error type: {other:?}"),
        Ok(_) => panic!("Expected connection to fail"),
    }
}

/// Test Unix domain socket improvements (when available)
#[tokio::test]
#[cfg(unix)]
async fn test_unix_socket_improvements() {
    use echosrv::unix::{Protocol, StreamExt};

    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    // Test connection using extension trait
    let connect_result = Protocol::connect_unix(&socket_path).await;

    // Connection should fail since no server is listening
    assert!(connect_result.is_err());

    // Test that the address parsing works
    let unix_addr: Address = socket_path.into();
    assert!(unix_addr.is_unix());
}

/// Benchmark-style test to verify performance characteristics
#[tokio::test]
async fn test_performance_characteristics() -> Result<(), Box<dyn std::error::Error>> {
    use tokio::net::TcpListener;

    // First bind to get the actual address
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener); // Close the listener so the server can bind to the same address

    let config = TcpConfig {
        bind_addr: addr,
        max_connections: 100,
        buffer_size: 8192,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = TcpEchoServer::new(config.clone().into());

    let server_handle = tokio::spawn(async move { server.run().await });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test with large data
    let large_data = vec![b'x'; 64 * 1024]; // 64KB
    let start = std::time::Instant::now();

    let mut client = TcpEchoClient::connect(addr).await?;
    let response = client.echo(&large_data).await?;

    let duration = start.elapsed();

    assert_eq!(response.len(), large_data.len());

    // Verify reasonable performance (this is a rough check)
    // 64KB should be processed in well under 1 second
    assert!(
        duration < Duration::from_millis(1000),
        "Large data processing took too long: {duration:?}"
    );

    server_handle.abort();

    Ok(())
}
