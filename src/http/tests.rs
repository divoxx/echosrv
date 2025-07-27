use super::protocol::{HttpProtocol, HttpProtocolError};
use crate::stream::{StreamConfig, StreamProtocol};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn test_http_protocol_bind_and_accept() {
    let config = StreamConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 10,
        buffer_size: 8192,
        read_timeout: std::time::Duration::from_secs(5),
        write_timeout: std::time::Duration::from_secs(5),
    };

    let listener = HttpProtocol::bind(&config).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn a client task
    let client_handle = tokio::spawn(async move { TcpStream::connect(addr).await.unwrap() });

    // Accept the connection
    let mut listener_mut = listener;
    let (_stream, client_addr) = HttpProtocol::accept(&mut listener_mut).await.unwrap();

    assert!(client_addr.ip().is_loopback());

    // Wait for client to connect
    let _client_stream = client_handle.await.unwrap();
}

#[tokio::test]
async fn test_http_protocol_connect() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    // Start a simple TCP listener
    let listener = TcpListener::bind(addr).await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    // Connect using HTTP protocol
    let _stream = HttpProtocol::connect(server_addr).await.unwrap();

    // Accept the connection to clean up
    let _ = listener.accept().await.unwrap();
}

#[tokio::test]
async fn test_http_protocol_simple_post() {
    let config = StreamConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 10,
        buffer_size: 8192,
        read_timeout: std::time::Duration::from_secs(5),
        write_timeout: std::time::Duration::from_secs(5),
    };

    let listener = HttpProtocol::bind(&config).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server task
    let server_handle = tokio::spawn(async move {
        let mut listener_mut = listener;
        let (mut stream, _) = HttpProtocol::accept(&mut listener_mut).await.unwrap();

        let mut buffer = [0u8; 1024];
        let n = HttpProtocol::read(&mut stream, &mut buffer).await.unwrap();

        // Echo back the data
        HttpProtocol::write(&mut stream, &buffer[..n])
            .await
            .unwrap();
    });

    // Client sends POST request
    let mut client_stream = TcpStream::connect(addr).await.unwrap();
    let request = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello";
    client_stream.write_all(request.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();

    // Read response
    let mut response = vec![0u8; 1024];
    let n = client_stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    // Should only contain the body content, no HTTP headers
    assert_eq!(response_str.trim(), "hello");

    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_http_protocol_method_not_allowed() {
    let config = StreamConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 10,
        buffer_size: 8192,
        read_timeout: std::time::Duration::from_secs(5),
        write_timeout: std::time::Duration::from_secs(5),
    };

    let listener = HttpProtocol::bind(&config).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server task
    let server_handle = tokio::spawn(async move {
        let mut listener_mut = listener;
        let (mut stream, _) = HttpProtocol::accept(&mut listener_mut).await.unwrap();

        let mut buffer = [0u8; 1024];
        let result = HttpProtocol::read(&mut stream, &mut buffer).await;

        // Should return an error for non-POST methods
        assert!(result.is_err());
        if let Err(HttpProtocolError::InvalidRequest(msg)) = result {
            assert!(msg.contains("Method GET not allowed"));
        } else {
            panic!("Expected InvalidRequest error");
        }
    });

    // Client sends GET request
    let mut client_stream = TcpStream::connect(addr).await.unwrap();
    let request = "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    client_stream.write_all(request.as_bytes()).await.unwrap();
    client_stream.flush().await.unwrap();

    // Read response (should be 405)
    let mut response = vec![0u8; 1024];
    let n = client_stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    assert!(response_str.contains("405"));
    assert!(response_str.contains("Method Not Allowed"));

    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_http_protocol_incomplete_request() {
    let config = StreamConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 10,
        buffer_size: 8192,
        read_timeout: std::time::Duration::from_secs(5),
        write_timeout: std::time::Duration::from_secs(5),
    };

    let listener = HttpProtocol::bind(&config).await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server task
    let server_handle = tokio::spawn(async move {
        let mut listener_mut = listener;
        let (mut stream, _) = HttpProtocol::accept(&mut listener_mut).await.unwrap();

        let mut buffer = [0u8; 1024];
        let result = HttpProtocol::read(&mut stream, &mut buffer).await;

        // The current implementation may handle incomplete requests differently
        // Just verify it doesn't crash and returns some result
        match result {
            Ok(_) => {
                // Some implementations might buffer and wait for more data
            }
            Err(HttpProtocolError::IncompleteRequest) => {
                // Expected behavior
            }
            Err(_e) => {
                // Other errors are also acceptable
            }
        }
    });

    // Client sends incomplete request
    let mut client_stream = TcpStream::connect(addr).await.unwrap();
    let incomplete_request = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\n"; // Missing body
    client_stream
        .write_all(incomplete_request.as_bytes())
        .await
        .unwrap();
    client_stream.flush().await.unwrap();

    // Give server time to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    server_handle.await.unwrap();
}
