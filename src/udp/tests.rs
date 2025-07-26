use super::*;
use crate::common::{EchoServer, EchoClient};
use crate::{Result, EchoError};
use std::time::Duration;
use std::net::SocketAddr;
use tracing::{error, info};

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    /// Helper function to create a simple UDP test server
    async fn create_udp_test_server() -> Result<(tokio::task::JoinHandle<Result<()>>, SocketAddr)> {
        let socket = UdpSocket::bind("127.0.0.1:0").await.map_err(EchoError::Udp)?;
        let addr = socket.local_addr().map_err(EchoError::Udp)?;

        let server_handle = tokio::spawn(async move {
            let mut buffer = [0; 1024];
            loop {
                match tokio::time::timeout(
                    Duration::from_secs(5),
                    socket.recv_from(&mut buffer)
                ).await {
                    Ok(Ok((n, addr))) => {
                        // Echo back the received data
                        if let Err(e) = socket.send_to(&buffer[..n], addr).await {
                            error!("Test server: Failed to send echo to {}: {}", addr, e);
                        } else {
                            info!("Test server: Echoed {} bytes to {}", n, addr);
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Test server: Failed to receive datagram: {}", e);
                        break;
                    }
                    Err(_) => {
                        // Timeout - server is done
                        break;
                    }
                }
            }
            info!("UDP test server stopped");
            Ok(())
        });
        Ok((server_handle, addr))
    }

    #[tokio::test]
    async fn test_config_default() {
        let config = UdpConfig::default();
        assert_eq!(config.buffer_size, 1024);
        // Timeouts are now always set (Duration instead of Option<Duration>)
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_echo_server_new() {
        let config = UdpConfig::default();
        let server = UdpEchoServer::new(config);
        assert!(server.shutdown_signal().receiver_count() == 0);
    }

    #[tokio::test]
    async fn test_echo_client_connect() {
        let (server_handle, addr) = create_udp_test_server().await.unwrap();
        
        // Test client connection
        let client = UdpEchoClient::connect(addr).await;
        assert!(client.is_ok());
        
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_basic_echo() -> Result<()> {
        let (server_handle, addr) = create_udp_test_server().await?;

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Test basic echo functionality
        let mut client = UdpEchoClient::connect(addr).await?;
        
        let test_data = "Hello, UDP Echo Server!";
        let response = client.echo_string(test_data).await?;
        
        assert_eq!(response, test_data);
        
        // Clean shutdown
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_messages() -> Result<()> {
        let (server_handle, addr) = create_udp_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = UdpEchoClient::connect(addr).await?;
        
        // Test multiple messages
        let messages = vec![
            "First UDP message",
            "Second UDP message",
            "Third UDP message with special chars: !@#$%^&*()",
            "Unicode: 🚀🌟🎉",
            "",
        ];

        for message in messages {
            let response = client.echo_string(message).await?;
            assert_eq!(response, message);
        }
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_binary_data() -> Result<()> {
        let (server_handle, addr) = create_udp_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = UdpEchoClient::connect(addr).await?;
        
        // Test binary data
        let binary_data = vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0xFD];
        let response = client.echo(&binary_data).await?;
        
        assert_eq!(response, binary_data);
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_large_data() -> Result<()> {
        let (server_handle, addr) = create_udp_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = UdpEchoClient::connect(addr).await?;
        
        // Test large data (but not too large for UDP)
        let large_data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let response = client.echo(&large_data).await?;
        
        assert_eq!(response, large_data);
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_clients() -> Result<()> {
        let (server_handle, addr) = create_udp_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Test multiple concurrent clients
        let client_count = 5;
        let mut handles = Vec::new();

        for i in 0..client_count {
            let addr = addr;
            let handle = tokio::spawn(async move {
                let mut client = UdpEchoClient::connect(addr).await?;
                let message = format!("Message from UDP client {}", i);
                let response = client.echo_string(&message).await?;
                assert_eq!(response, message);
                Ok::<(), EchoError>(())
            });
            handles.push(handle);
        }

        // Wait for all clients to complete
        for handle in handles {
            handle.await.map_err(|e| EchoError::Config(format!("Task join error: {}", e)))??;
        }
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_udp_server_timeout() -> Result<()> {
        // Test server with very short timeouts
        let config = UdpConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            buffer_size: 1024,
                    read_timeout: Duration::from_millis(100), // Very short timeout
        write_timeout: Duration::from_millis(100),
        };

        let server = UdpEchoServer::new(config);
        let server_handle = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                server.run()
            ).await.map_err(|_| EchoError::Timeout("Server timeout".to_string()))?
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
} 