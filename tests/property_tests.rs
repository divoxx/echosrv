use echosrv::{TcpEchoServer, TcpEchoClient, TcpConfig, EchoClient, EchoServerTrait};
use echosrv::common::create_controlled_test_server_with_limit;
use proptest::prelude::*;
use std::time::Duration;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Property: Echo server should return exactly the same data that was sent
    #[test]
    fn echo_preserves_data(data in prop::collection::vec(any::<u8>(), 0..1024)) {
        tokio_test::block_on(async {
            if data.is_empty() {
                return Ok(()); // Skip empty data test
            }

            // Create a test server
            let (server_handle, addr) = create_controlled_test_server_with_limit(10).await
                .map_err(|e| TestCaseError::fail(format!("Server setup failed: {}", e)))?;

            // Give server time to start
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Connect client and test echo
            let mut client = TcpEchoClient::connect(addr).await
                .map_err(|e| TestCaseError::fail(format!("Client connection failed: {}", e)))?;

            let response = client.echo(&data).await
                .map_err(|e| TestCaseError::fail(format!("Echo failed: {}", e)))?;

            // Clean shutdown
            server_handle.abort();

            // Property: response should be identical to input
            prop_assert_eq!(response, data);
            Ok(())
        })?;
    }

    /// Property: Echo server should handle different string encodings correctly
    #[test]
    fn echo_preserves_strings(text in ".*") {
        tokio_test::block_on(async {
            if text.is_empty() {
                return Ok(()); // Skip empty string test
            }

            let (server_handle, addr) = create_controlled_test_server_with_limit(10).await
                .map_err(|e| TestCaseError::fail(format!("Server setup failed: {}", e)))?;

            tokio::time::sleep(Duration::from_millis(50)).await;

            let mut client = TcpEchoClient::connect(addr).await
                .map_err(|e| TestCaseError::fail(format!("Client connection failed: {}", e)))?;

            let response = client.echo_string(&text).await
                .map_err(|e| TestCaseError::fail(format!("Echo string failed: {}", e)))?;

            server_handle.abort();

            // Property: response string should be identical to input
            prop_assert_eq!(response, text);
            Ok(())
        })?;
    }

    /// Property: Multiple concurrent clients should all receive correct responses
    #[test]
    fn concurrent_clients_get_correct_responses(
        messages in prop::collection::vec(".*", 1..10)
    ) {
        tokio_test::block_on(async {
            let (server_handle, addr) = create_controlled_test_server_with_limit(20).await
                .map_err(|e| TestCaseError::fail(format!("Server setup failed: {}", e)))?;

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Create concurrent client tasks
            let mut handles = Vec::new();
            for (i, message) in messages.iter().enumerate() {
                if message.is_empty() {
                    continue; // Skip empty messages
                }
                
                let addr = addr;
                let message = message.clone();
                let handle = tokio::spawn(async move {
                    let mut client = TcpEchoClient::connect(addr).await?;
                    let response = client.echo_string(&message).await?;
                    Ok::<(String, String), echosrv::EchoError>((message, response))
                });
                handles.push((i, handle));
            }

            // Collect all responses
            let mut results = Vec::new();
            for (i, handle) in handles {
                let result = handle.await
                    .map_err(|e| TestCaseError::fail(format!("Task {} join error: {}", i, e)))?
                    .map_err(|e| TestCaseError::fail(format!("Client {} error: {}", i, e)))?;
                results.push(result);
            }

            server_handle.abort();

            // Property: each client should receive back exactly what it sent
            for (original, response) in results {
                prop_assert_eq!(original, response);
            }

            Ok(())
        })?;
    }

    /// Property: Server should handle various buffer sizes correctly
    #[test]
    fn handles_different_buffer_sizes(
        data in prop::collection::vec(any::<u8>(), 1..8192),
        buffer_size in 64usize..2048
    ) {
        tokio_test::block_on(async {
            use tokio::net::TcpListener;
            
            // First bind to get the actual address
            let listener = TcpListener::bind("127.0.0.1:0").await
                .map_err(|e| TestCaseError::fail(format!("Failed to bind listener: {}", e)))?;
            let addr = listener.local_addr()
                .map_err(|e| TestCaseError::fail(format!("Failed to get local address: {}", e)))?;
            drop(listener); // Close the listener so the server can bind to the same address
            
            // Create server with custom buffer size
            let config = TcpConfig {
                bind_addr: addr,
                max_connections: 10,
                buffer_size,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
            };

            let server = TcpEchoServer::new(config.clone().into());
            let server_handle = tokio::spawn(async move {
                server.run().await
            });

            // Give server time to start
            tokio::time::sleep(Duration::from_millis(100)).await;

            let mut client = TcpEchoClient::connect(addr).await
                .map_err(|e| TestCaseError::fail(format!("Client connection failed: {}", e)))?;

            let response = client.echo(&data).await
                .map_err(|e| TestCaseError::fail(format!("Echo failed: {}", e)))?;

            server_handle.abort();

            // Property: response should match input regardless of buffer size
            prop_assert_eq!(response, data);
            Ok(())
        })?;
    }
}

/// Stress test with many connections
#[tokio::test]
async fn stress_test_many_connections() {
    
    let (server_handle, addr) = create_controlled_test_server_with_limit(100).await.unwrap();
    
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create 50 concurrent connections
    let mut handles = Vec::new();
    for i in 0..50 {
        let addr = addr;
        let handle = tokio::spawn(async move {
            let mut client = TcpEchoClient::connect(addr).await?;
            let message = format!("Stress test message from client {}", i);
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), echosrv::EchoError>(())
        });
        handles.push(handle);
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    server_handle.abort();
}

/// Test rapid connect/disconnect cycles
#[tokio::test]
async fn rapid_connect_disconnect() {
    let (server_handle, addr) = create_controlled_test_server_with_limit(50).await.unwrap();
    
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Rapidly connect and disconnect
    for i in 0..20 {
        let mut client = TcpEchoClient::connect(addr).await.unwrap();
        let message = format!("Rapid test {}", i);
        let response = client.echo_string(&message).await.unwrap();
        assert_eq!(response, message);
        drop(client); // Explicit disconnect
        
        // Small delay to avoid overwhelming the server
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    server_handle.abort();
}

/// Test with binary data containing null bytes
#[tokio::test]
async fn binary_data_with_nulls() {
    let (server_handle, addr) = create_controlled_test_server_with_limit(10).await.unwrap();
    
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut client = TcpEchoClient::connect(addr).await.unwrap();
    
    // Binary data with null bytes and various patterns
    let test_data = vec![
        vec![0, 1, 2, 3, 0, 255, 128, 0],
        vec![255; 100],
        vec![0; 100],
        (0..=255).collect::<Vec<u8>>(),
    ];

    for data in test_data {
        let response = client.echo(&data).await.unwrap();
        assert_eq!(response, data);
    }

    server_handle.abort();
}