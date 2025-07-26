use crate::unix::{
    UnixStreamEchoServer, UnixStreamEchoClient,
    UnixDatagramEchoServer, UnixDatagramEchoClient,
    UnixStreamConfig, UnixDatagramConfig,
};
use crate::common::{EchoServerTrait, EchoClient};
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn test_unix_stream_echo() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_stream.sock");
    
    let config = UnixStreamConfig {
        socket_path: socket_path.clone(),
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
    };
    
    let server = UnixStreamEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();
    
    // Start server in background
    let server_handle = tokio::spawn(async move {
        server.run().await
    });
    
    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Test client
    let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();
    
    // Test string echo
    let test_string = "Hello, Unix Stream!";
    let response = client.echo_string(test_string).await.unwrap();
    assert_eq!(response, test_string);
    
    // Test binary data echo
    let test_data = b"Binary data with \x00 null bytes";
    let response = client.echo(test_data).await.unwrap();
    assert_eq!(response, test_data);
    
    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_unix_datagram_echo() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_datagram.sock");
    
    let config = UnixDatagramConfig {
        socket_path: socket_path.clone(),
        buffer_size: 1024,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
    };
    
    let server = UnixDatagramEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();
    
    // Start server in background
    let server_handle = tokio::spawn(async move {
        server.run().await
    });
    
    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Test client with timeout
    let client_result = tokio::time::timeout(
        Duration::from_secs(10),
        async {
            let mut client = UnixDatagramEchoClient::connect(socket_path).await.unwrap();
            
            // Test string echo
            let test_string = "Hello, Unix Datagram!";
            let response = client.echo_string(test_string).await.unwrap();
            assert_eq!(response, test_string);
            
            // Test binary data echo
            let test_data = b"Binary data with \x00 null bytes";
            let response = client.echo(test_data).await.unwrap();
            assert_eq!(response, test_data);
        }
    ).await;
    
    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
    
    // Check if client test passed
    client_result.unwrap();
}

#[tokio::test]
async fn test_unix_stream_multiple_clients() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_multi_stream.sock");
    
    let config = UnixStreamConfig {
        socket_path: socket_path.clone(),
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
    };
    
    let server = UnixStreamEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();
    
    // Start server in background
    let server_handle = tokio::spawn(async move {
        server.run().await
    });
    
    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Test multiple concurrent clients
    let mut handles = Vec::new();
    
    for i in 0..5 {
        let socket_path = socket_path.clone();
        let handle = tokio::spawn(async move {
            let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();
            let test_string = format!("Client {}", i);
            let response = client.echo_string(&test_string).await.unwrap();
            assert_eq!(response, test_string);
        });
        handles.push(handle);
    }
    
    // Wait for all clients to complete
    for handle in handles {
        handle.await.unwrap();
    }
    
    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_unix_stream_large_data() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("test_large_stream.sock");
    
    let config = UnixStreamConfig {
        socket_path: socket_path.clone(),
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
    };
    
    let server = UnixStreamEchoServer::new(config);
    let shutdown_signal = server.shutdown_signal();
    
    // Start server in background
    let server_handle = tokio::spawn(async move {
        server.run().await
    });
    
    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Test client with large data
    let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();
    
    // Create large test data (larger than buffer size)
    let large_data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
    let response = client.echo(&large_data).await.unwrap();
    assert_eq!(response, large_data);
    
    // Shutdown server
    let _ = shutdown_signal.send(());
    server_handle.await.unwrap().unwrap();
} 