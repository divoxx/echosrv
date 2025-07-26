use super::*;
use crate::common::{create_test_server, EchoClient, EchoServerTrait};
use crate::TcpEchoServer;
use crate::{Result, EchoError};
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{EchoClient, EchoServerTrait};
    use crate::TcpEchoServer;

    #[tokio::test]
    async fn test_config_default() {
        let config = TcpConfig::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.buffer_size, 1024);
        // Timeouts are now always set (Duration instead of Option<Duration>)
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_echo_server_new() {
        let config = TcpConfig::default();
        let server = TcpEchoServer::new(config);
        assert!(server.shutdown_signal().receiver_count() == 0);
    }

    #[tokio::test]
    async fn test_echo_client_connect() {
        let (server_handle, addr) = create_test_server().await.unwrap();
        
        // Test client connection
        let client = TcpEchoClient::connect(addr).await;
        assert!(client.is_ok());
        
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_basic_echo() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Test basic echo functionality
        let mut client = TcpEchoClient::connect(addr).await?;
        
        let test_data = "Hello, Echo Server!";
        let response = client.echo_string(test_data).await?;
        
        assert_eq!(response, test_data);
        
        // Clean shutdown
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_messages() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = TcpEchoClient::connect(addr).await?;
        
        // Test multiple messages
        let messages = vec![
            "First message",
            "Second message",
            "Third message with special chars: !@#$%^&*()",
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
        let (server_handle, addr) = create_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = TcpEchoClient::connect(addr).await?;
        
        // Test binary data
        let binary_data = vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0xFD];
        let response = client.echo(&binary_data).await?;
        
        assert_eq!(response, binary_data);
        
        server_handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_large_data() -> Result<()> {
        let (server_handle, addr) = create_test_server().await?;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut client = TcpEchoClient::connect(addr).await?;
        
        // Test large data (larger than buffer size)
        let large_data: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
        let response = client.echo(&large_data).await?;
        
        assert_eq!(response, large_data);
        
        server_handle.abort();
        Ok(())
    }
} 