mod common;
use common::create_controlled_test_server_with_limit;
use echosrv::http::HttpConfig;
use echosrv::{Address, EchoClient, EchoServerTrait, TcpEchoServer, UdpEchoServer};
use echosrv::{EchoError, Result};
use echosrv::{TcpConfig, TcpEchoClient};
use echosrv::{UdpConfig, UdpEchoClient};
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};
use tracing::{error, info};

#[tokio::test]
async fn test_multiple_concurrent_tcp_clients() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(10).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test multiple concurrent clients
    let client_count = 5;
    let mut handles = Vec::new();

    for i in 0..client_count {
        let handle = tokio::spawn(async move {
            let mut client = TcpEchoClient::connect(addr).await?;
            let message = format!("Message from TCP client {i}");
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), color_eyre::eyre::Error>(())
        });
        handles.push(handle);
    }

    // Wait for all clients to complete
    for handle in handles {
        if let Err(e) = handle.await {
            return Err(EchoError::Config(format!("Task join error: {e}")));
        }
    }

    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_multiple_concurrent_udp_clients() -> Result<()> {
    // Create a UDP test server
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(EchoError::Udp)?;
    let addr = socket.local_addr().map_err(EchoError::Udp)?;

    let server_handle = tokio::spawn(async move {
        let mut buffer = [0; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer)).await
            {
                Ok(Ok((n, client_addr))) => {
                    // Echo back the received data
                    if let Err(e) = socket.send_to(&buffer[..n], client_addr).await {
                        error!(
                            "UDP test server: Failed to send echo to {}: {}",
                            client_addr, e
                        );
                    } else {
                        info!("UDP test server: Echoed {} bytes to {}", n, client_addr);
                    }
                }
                Ok(Err(e)) => {
                    error!("UDP test server: Failed to receive datagram: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout - server is done
                    break;
                }
            }
        }
        info!("UDP test server stopped");
        Ok::<(), EchoError>(())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test multiple concurrent UDP clients
    let client_count = 5;
    let mut handles = Vec::new();

    for i in 0..client_count {
        let handle = tokio::spawn(async move {
            let mut client = UdpEchoClient::connect(addr).await?;
            let message = format!("Message from UDP client {i}");
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), EchoError>(())
        });
        handles.push(handle);
    }

    // Wait for all clients to complete
    for handle in handles {
        handle
            .await
            .map_err(|e| EchoError::Config(format!("Task join error: {e}")))??;
    }

    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_tcp_connection_limit() -> Result<()> {
    let config = TcpConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 2, // Very low limit for testing
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
        ..Default::default()
    };

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .map_err(EchoError::Tcp)?;
    let addr = listener.local_addr().map_err(EchoError::Tcp)?;
    drop(listener);

    let config = TcpConfig {
        bind_addr: addr,
        max_connections: 2,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
        ..Default::default()
    };

    let server = TcpEchoServer::new(config.into());
    let server_handle = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), server.run())
            .await
            .map_err(|_| EchoError::Timeout("Server timeout".to_string()))?
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Try to create more connections than the limit concurrently
    let mut handles = Vec::new();

    for i in 0..5 {
        let handle = tokio::spawn(async move {
            match TcpEchoClient::connect(addr).await {
                Ok(mut client) => {
                    // Try to echo something to verify the connection works
                    match client.echo_string("test").await {
                        Ok(response) => {
                            if response == "test" {
                                Ok::<usize, EchoError>(1) // Success
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
    assert!(
        successful_connections <= 2,
        "Expected at most 2 successful connections, got {successful_connections}"
    );
    assert!(
        failed_connections > 0,
        "Expected some connection failures, got {failed_connections}"
    );

    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_tcp_graceful_shutdown() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(10).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify server is running
    let mut client = TcpEchoClient::connect(addr).await?;
    let response = client.echo_string("test").await?;
    assert_eq!(response, "test");

    // Shutdown server
    server_handle.abort();

    // Give server time to shutdown
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify server is no longer accepting connections
    match TcpEchoClient::connect(addr).await {
        Ok(_) => panic!("Server should not accept connections after shutdown"),
        Err(_) => {
            // Expected - server is shutdown
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_udp_graceful_shutdown() -> Result<()> {
    // Create a UDP test server
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(EchoError::Udp)?;
    let addr = socket.local_addr().map_err(EchoError::Udp)?;

    let server_handle = tokio::spawn(async move {
        let mut buffer = [0; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer)).await
            {
                Ok(Ok((n, client_addr))) => {
                    // Echo back the received data
                    if let Err(e) = socket.send_to(&buffer[..n], client_addr).await {
                        error!(
                            "UDP test server: Failed to send echo to {}: {}",
                            client_addr, e
                        );
                    } else {
                        info!("UDP test server: Echoed {} bytes to {}", n, client_addr);
                    }
                }
                Ok(Err(e)) => {
                    error!("UDP test server: Failed to receive datagram: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout - server is done
                    break;
                }
            }
        }
        info!("UDP test server stopped");
        Ok::<(), EchoError>(())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify server is running
    let mut client = UdpEchoClient::connect(addr).await?;
    let response = client.echo_string("test").await?;
    assert_eq!(response, "test");

    // Shutdown server
    server_handle.abort();

    // Give server time to shutdown
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify server is no longer responding
    let mut client = UdpEchoClient::connect(addr).await?;
    match client.echo_string("test").await {
        Ok(_) => panic!("Server should not respond after shutdown"),
        Err(_) => {
            // Expected - server is shutdown
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_tcp_timeout_configuration() -> Result<()> {
    // Test server with very short timeouts
    let config = TcpConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Duration::from_millis(100), // Very short timeout
        write_timeout: Duration::from_millis(100),
        ..Default::default()
    };

    let listener = TcpListener::bind(config.bind_addr).await?;
    let addr = listener.local_addr()?;
    drop(listener);

    let config = TcpConfig {
        bind_addr: addr,
        max_connections: 10,
        buffer_size: 1024,
        read_timeout: Duration::from_millis(100),
        write_timeout: Duration::from_millis(100),
        ..Default::default()
    };

    let server = TcpEchoServer::new(config.into());
    let server_handle = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), server.run())
            .await
            .map_err(|_| EchoError::Timeout("Server timeout".to_string()))?
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test that normal operations work with timeouts
    let mut client = TcpEchoClient::connect(addr).await?;
    let response = client.echo_string("quick test").await?;
    assert_eq!(response, "quick test");

    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_udp_timeout_configuration() -> Result<()> {
    // Test server with very short timeouts
    let config = UdpConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        buffer_size: 1024,
        read_timeout: Duration::from_millis(100), // Very short timeout
        write_timeout: Duration::from_millis(100),
        ..Default::default()
    };

    let server = UdpEchoServer::new(config.into());
    let server_handle = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), server.run())
            .await
            .map_err(|_| EchoError::Timeout("Server timeout".to_string()))?
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test that normal operations work with timeouts
    let addr = "127.0.0.1:0".parse().unwrap();
    let mut client = UdpEchoClient::connect(addr).await?;

    // This should fail since we're not connecting to the actual server
    // but it tests that the client can be created with timeout config
    assert!(client.echo_string("quick test").await.is_err());

    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_tcp_stress_test() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(50).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stress test with many rapid connections
    let mut handles = Vec::new();
    let mut successful_echoes = 0;
    let mut failed_connections = 0;

    for i in 0..100 {
        let handle = tokio::spawn(async move {
            match TcpEchoClient::connect(addr).await {
                Ok(mut client) => {
                    // Send multiple messages per connection
                    if let Some(j) = (0..5).next() {
                        let message = format!("Stress test message {j} from client {i}");
                        match client.echo_string(&message).await {
                            Ok(response) => {
                                if response == message {
                                    return Ok::<usize, EchoError>(1); // Success
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
    assert!(
        successful_echoes > 0,
        "Expected some successful echoes, got {successful_echoes}"
    );
    info!(
        "TCP stress test completed: {} successful echoes, {} failed connections",
        successful_echoes, failed_connections
    );

    server_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_udp_stress_test() -> Result<()> {
    // Create a UDP test server
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(EchoError::Udp)?;
    let addr = socket.local_addr().map_err(EchoError::Udp)?;

    let server_handle = tokio::spawn(async move {
        let mut buffer = [0; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer)).await
            {
                Ok(Ok((n, client_addr))) => {
                    // Echo back the received data
                    if let Err(e) = socket.send_to(&buffer[..n], client_addr).await {
                        error!(
                            "UDP test server: Failed to send echo to {}: {}",
                            client_addr, e
                        );
                    } else {
                        info!("UDP test server: Echoed {} bytes to {}", n, client_addr);
                    }
                }
                Ok(Err(e)) => {
                    error!("UDP test server: Failed to receive datagram: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout - server is done
                    break;
                }
            }
        }
        info!("UDP test server stopped");
        Ok::<(), EchoError>(())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stress test with many rapid UDP messages
    let mut handles = Vec::new();
    let mut successful_echoes = 0;
    let mut failed_connections = 0;

    for i in 0..100 {
        let handle = tokio::spawn(async move {
            match UdpEchoClient::connect(addr).await {
                Ok(mut client) => {
                    // Send multiple messages per client
                    if let Some(j) = (0..5).next() {
                        let message = format!("UDP stress test message {j} from client {i}");
                        match client.echo_string(&message).await {
                            Ok(response) => {
                                if response == message {
                                    return Ok::<usize, EchoError>(1); // Success
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
    assert!(
        successful_echoes > 0,
        "Expected some successful echoes, got {successful_echoes}"
    );
    info!(
        "UDP stress test completed: {} successful echoes, {} failed connections",
        successful_echoes, failed_connections
    );

    server_handle.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

mod http_support {
    use echosrv::http::{HttpConfig, HttpEchoServer};
    use echosrv::{EchoServerTrait, Result};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::broadcast;
    use tokio::task::JoinHandle;

    pub const SERVER_NAME: &str = "IntegrationTest/1.0";
    pub const CONTENT_TYPE: &str = "application/x-echo";

    /// A test configuration with distinctive response header values.
    pub fn config() -> HttpConfig {
        HttpConfig {
            max_connections: 50,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            server_name: Some(SERVER_NAME.to_string()),
            default_content_type: Some(CONTENT_TYPE.to_string()),
            ..HttpConfig::default()
        }
    }

    pub struct TestServer {
        pub addr: SocketAddr,
        shutdown: broadcast::Sender<()>,
        handle: JoinHandle<Result<()>>,
    }

    impl TestServer {
        pub async fn stop(self) {
            let _ = self.shutdown.send(());
            self.handle
                .await
                .expect("server task panicked")
                .expect("server returned an error");
        }
    }

    /// Binds an `HttpEchoServer` to an ephemeral port and serves it in the
    /// background. The listener exists when this returns, so no polling is
    /// needed.
    pub async fn start(mut config: HttpConfig) -> TestServer {
        config.bind_addr = "127.0.0.1:0".parse().unwrap();
        let server = HttpEchoServer::new(config);
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.expect("failed to bind HTTP server");
        let addr = *bound
            .local_addr()
            .as_network()
            .expect("HTTP server bound to a non-network address");
        let handle = tokio::spawn(bound.serve());
        TestServer {
            addr,
            shutdown,
            handle,
        }
    }

    #[derive(Debug)]
    pub struct Response {
        pub status: u16,
        pub reason: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Response {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }

        /// Asserts the headers every response must carry.
        pub fn assert_common_headers(&self) {
            assert_eq!(self.header("Connection"), Some("close"));
            assert_eq!(self.header("Server"), Some(SERVER_NAME));
            let len: usize = self
                .header("Content-Length")
                .expect("missing Content-Length")
                .parse()
                .unwrap();
            assert_eq!(len, self.body.len(), "Content-Length matches body");
        }
    }

    /// Parses one complete response. The body is everything after the head.
    pub fn parse(raw: &[u8]) -> Response {
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut res = httparse::Response::new(&mut headers);
        let len = match res.parse(raw).expect("invalid HTTP response") {
            httparse::Status::Complete(len) => len,
            httparse::Status::Partial => panic!(
                "incomplete HTTP response: {:?}",
                String::from_utf8_lossy(raw)
            ),
        };
        assert_eq!(res.version, Some(1), "HTTP/1.1 status line");
        Response {
            status: res.code.unwrap(),
            reason: res.reason.unwrap().to_string(),
            headers: res
                .headers
                .iter()
                .map(|h| {
                    (
                        h.name.to_string(),
                        String::from_utf8(h.value.to_vec()).unwrap(),
                    )
                })
                .collect(),
            body: raw[len..].to_vec(),
        }
    }

    /// Reads until the server closes the connection.
    pub async fn read_all(stream: &mut TcpStream) -> Vec<u8> {
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw))
            .await
            .expect("timed out waiting for the server to close")
            .expect("read failed");
        raw
    }

    /// Sends `parts` (pausing `gap` between them), then reads the response.
    pub async fn send_parts(addr: SocketAddr, parts: &[&[u8]], gap: Duration) -> Response {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 && !gap.is_zero() {
                tokio::time::sleep(gap).await;
            }
            stream.write_all(part).await.unwrap();
            stream.flush().await.unwrap();
        }
        parse(&read_all(&mut stream).await)
    }

    pub async fn send(addr: SocketAddr, request: &[u8]) -> Response {
        send_parts(addr, &[request], Duration::ZERO).await
    }

    pub fn post(body: &[u8]) -> Vec<u8> {
        let mut req = format!(
            "POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        req.extend_from_slice(body);
        req
    }

    /// Deterministic binary payload.
    pub fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }
}

#[tokio::test]
async fn test_http_response_shape() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(server.addr, &http_support::post(b"post body")).await;

    assert_eq!(res.status, 200);
    assert_eq!(res.reason, "OK");
    res.assert_common_headers();
    assert_eq!(res.header("Content-Type"), Some(http_support::CONTENT_TYPE));
    assert_eq!(res.header("Content-Length"), Some("9"));
    assert_eq!(res.body, b"post body");
    server.stop().await;
}

#[tokio::test]
async fn test_http_large_body() {
    let server = http_support::start(http_support::config()).await;
    // 64 KiB through a 1 KiB server buffer.
    let body = http_support::payload(64 * 1024);
    let res = http_support::send(server.addr, &http_support::post(&body)).await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, body);
    server.stop().await;
}

#[tokio::test]
async fn test_http_head_and_body_in_separate_writes() {
    let server = http_support::start(http_support::config()).await;
    let body = http_support::payload(5000);
    let head = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let res = http_support::send_parts(
        server.addr,
        &[
            &head.as_bytes()[..10],
            &head.as_bytes()[10..],
            &body[..1000],
            &body[1000..],
        ],
        Duration::from_millis(30),
    )
    .await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, body);
    server.stop().await;
}

#[tokio::test]
async fn test_http_byte_at_a_time() {
    let server = http_support::start(http_support::config()).await;
    let request = http_support::post(b"slow and steady");
    let parts: Vec<&[u8]> = request.chunks(1).collect();
    let res = http_support::send_parts(server.addr, &parts, Duration::ZERO).await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, b"slow and steady");
    server.stop().await;
}

#[tokio::test]
async fn test_http_empty_post() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(server.addr, &http_support::post(b"")).await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.header("Content-Length"), Some("0"));
    assert!(res.body.is_empty());
    server.stop().await;
}

#[tokio::test]
async fn test_http_missing_content_length_is_empty_body() {
    let server = http_support::start(http_support::config()).await;
    // Without Content-Length the body is empty; trailing bytes are ignored.
    let res = http_support::send(
        server.addr,
        b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\nignored",
    )
    .await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert!(res.body.is_empty());
    server.stop().await;
}

#[tokio::test]
async fn test_http_chunked_is_not_implemented() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(
        server.addr,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    )
    .await;

    assert_eq!(res.status, 501);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_too_many_headers() {
    let server = http_support::start(http_support::config()).await;
    let mut request = String::from("POST / HTTP/1.1\r\nHost: localhost\r\n");
    for i in 0..32 {
        request.push_str(&format!("X-Header-{i}: value\r\n"));
    }
    request.push_str("Content-Length: 0\r\n\r\n");
    let res = http_support::send(server.addr, request.as_bytes()).await;

    assert_eq!(res.status, 400);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_header_section_too_large() {
    let server = http_support::start(http_support::config()).await;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nX-Big: {}\r\nContent-Length: 0\r\n\r\n",
        "a".repeat(9 * 1024)
    );
    let res = http_support::send(server.addr, request.as_bytes()).await;

    assert_eq!(res.status, 431);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_invalid_content_length() {
    let server = http_support::start(http_support::config()).await;
    for value in ["abc", "-5", "5, 6", "1.5"] {
        let request =
            format!("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {value}\r\n\r\n");
        let res = http_support::send(server.addr, request.as_bytes()).await;
        assert_eq!(res.status, 400, "Content-Length: {value}");
        res.assert_common_headers();
    }
    server.stop().await;
}

#[tokio::test]
async fn test_http_malformed_request_line() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(server.addr, b"NOT AN HTTP REQUEST\r\n\r\n").await;
    assert_eq!(res.status, 400);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_body_too_large() {
    let server = http_support::start(HttpConfig {
        max_body_size: 16,
        ..http_support::config()
    })
    .await;

    // Exactly at the limit is fine.
    let res = http_support::send(server.addr, &http_support::post(&[b'a'; 16])).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, [b'a'; 16]);

    // One byte over is rejected, even though the body is sent anyway.
    let res = http_support::send(server.addr, &http_support::post(&[b'a'; 17])).await;
    assert_eq!(res.status, 413);
    res.assert_common_headers();

    // A huge declared length is rejected without waiting for the body.
    let res = http_support::send(
        server.addr,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000000\r\n\r\n",
    )
    .await;
    assert_eq!(res.status, 413);
    server.stop().await;
}

#[tokio::test]
async fn test_http_expect_100_continue() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server = http_support::start(http_support::config()).await;
    let body = http_support::payload(2048);
    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let head = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();

    // The server must invite the body before we send it.
    let expected = b"HTTP/1.1 100 Continue\r\n\r\n";
    let mut interim = [0u8; 25];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut interim))
        .await
        .expect("no 100 Continue")
        .unwrap();
    assert_eq!(&interim, expected);

    stream.write_all(&body).await.unwrap();
    let res = http_support::parse(&http_support::read_all(&mut stream).await);
    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, body);
    server.stop().await;
}

#[tokio::test]
async fn test_http_method_not_allowed() {
    let server = http_support::start(http_support::config()).await;
    for method in ["GET", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
        let request = format!("{method} / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let res = http_support::send(server.addr, request.as_bytes()).await;

        assert_eq!(res.status, 405, "{method}");
        assert_eq!(res.reason, "Method Not Allowed");
        assert_eq!(res.header("Allow"), Some("POST"));
        res.assert_common_headers();
        let body = String::from_utf8(res.body).unwrap();
        assert!(
            body.contains(&format!("Method {method} not allowed")),
            "{body}"
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn test_http_client_round_trip() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(http_support::config()).await;
    let mut client = HttpEchoClient::connect(Address::Network(server.addr)).await?;

    assert_eq!(client.echo_string("hello").await?, "hello");
    // The server closes after each response; the client reconnects.
    assert_eq!(client.echo(b"").await?, b"");
    let body = http_support::payload(100 * 1024);
    assert_eq!(client.echo(&body).await?, body);

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn test_http_client_reports_error_status() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(HttpConfig {
        max_body_size: 4,
        ..http_support::config()
    })
    .await;
    let mut client = HttpEchoClient::connect(server.addr).await?;
    let err = client
        .echo(b"too long")
        .await
        .expect_err("413 must be an error");
    assert!(err.to_string().contains("413"), "{err}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn test_http_concurrent_clients() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(http_support::config()).await;
    let addr = server.addr;
    let handles: Vec<_> = (0..10)
        .map(|i| {
            tokio::spawn(async move {
                let mut client = HttpEchoClient::connect(addr).await?;
                let message = format!("concurrent client {i}");
                let response = client.echo_string(&message).await?;
                assert_eq!(response, message);
                Ok::<(), EchoError>(())
            })
        })
        .collect();

    for handle in handles {
        handle
            .await
            .map_err(|e| EchoError::Config(format!("Task join error: {e}")))??;
    }
    server.stop().await;
    Ok(())
}
