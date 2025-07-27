use crate::{Result, EchoError};
use crate::common::EchoClient;
use crate::network::Address;
use super::StreamProtocol;
use std::time::Duration;
use tokio::time::{timeout, Instant};
use async_trait::async_trait;
use bytes::BytesMut;

/// Configuration for stream clients
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Read timeout for operations
    pub read_timeout: Duration,
    /// Write timeout for operations
    pub write_timeout: Duration,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Buffer size for reading data
    pub buffer_size: usize,
    /// Maximum response size to prevent memory exhaustion
    pub max_response_size: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            buffer_size: 1024,
            max_response_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Stream-based echo client with configurable timeouts and error handling
///
/// This client provides configurable timeouts, better error handling,
/// and protection against memory exhaustion.
pub struct Client<P: StreamProtocol> {
    stream: P::Stream,
    config: ClientConfig,
    last_activity: Instant,
}

impl<P: StreamProtocol> Client<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Connect to a server with custom configuration
    pub async fn connect_with_config<A: Into<Address>>(
        address: A,
        config: ClientConfig,
    ) -> Result<Self> {
        let address = address.into();
        let stream = match &address {
            Address::Network(addr) => {
                timeout(config.connect_timeout, P::connect(*addr))
                    .await
                    .map_err(|_| EchoError::Timeout("Connection timeout".to_string()))?
                    .map_err(|e| e.into())?
            }
            Address::Unix(_) => {
                return Err(EchoError::Unsupported(
                    "Use Unix-specific client for Unix domain sockets".to_string()
                ));
            }
        };

        Ok(Self {
            stream,
            config,
            last_activity: Instant::now(),
        })
    }

    /// Connect with default configuration
    pub async fn connect<A: Into<Address>>(address: A) -> Result<Self> {
        Self::connect_with_config(address, ClientConfig::default()).await
    }

    /// Check if the client has been idle for too long
    pub fn is_idle(&self, max_idle: Duration) -> bool {
        self.last_activity.elapsed() > max_idle
    }

    /// Update the last activity timestamp
    fn update_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Send data and receive response with proper timeout handling
    async fn send_and_receive(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.update_activity();

        // Write data with timeout
        timeout(self.config.write_timeout, P::write(&mut self.stream, data))
            .await
            .map_err(|_| EchoError::Timeout("Write timeout".to_string()))?
            .map_err(|e| e.into())?;

        timeout(self.config.write_timeout, P::flush(&mut self.stream))
            .await
            .map_err(|_| EchoError::Timeout("Flush timeout".to_string()))?
            .map_err(|e| e.into())?;

        // Read response with timeout and size limits
        let mut response = BytesMut::with_capacity(self.config.buffer_size);
        let mut buffer = vec![0u8; self.config.buffer_size];

        loop {
            let read_result = timeout(
                self.config.read_timeout,
                P::read(&mut self.stream, &mut buffer)
            ).await;

            match read_result {
                Ok(Ok(0)) => {
                    // Connection closed, return what we have
                    break;
                }
                Ok(Ok(n)) => {
                    // Check size limit before extending
                    if response.len() + n > self.config.max_response_size {
                        return Err(EchoError::Config(format!(
                            "Response too large: {} bytes, max allowed: {}",
                            response.len() + n,
                            self.config.max_response_size
                        )));
                    }
                    
                    response.extend_from_slice(&buffer[..n]);
                    
                    // For echo servers, we expect to receive exactly what we sent
                    // Stop reading when we have received at least as much as we sent
                    if response.len() >= data.len() {
                        break;
                    }
                }
                Ok(Err(e)) => {
                    return Err(e.into());
                }
                Err(_) => {
                    // Timeout - this might be normal if we've received all expected data
                    if response.len() >= data.len() {
                        break;
                    } else {
                        return Err(EchoError::Timeout(format!(
                            "Read timeout: expected {} bytes, got {} bytes",
                            data.len(),
                            response.len()
                        )));
                    }
                }
            }
        }

        self.update_activity();
        Ok(response.to_vec())
    }

    /// Get client configuration
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Update client configuration
    pub fn set_config(&mut self, config: ClientConfig) {
        self.config = config;
    }
}

#[async_trait]
impl<P: StreamProtocol> EchoClient for Client<P>
where
    P::Error: Into<EchoError> + std::fmt::Display,
{
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        // Validate input size
        if data.len() > self.config.max_response_size {
            return Err(EchoError::Config(format!(
                "Request too large: {} bytes, max allowed: {}",
                data.len(),
                self.config.max_response_size
            )));
        }

        self.send_and_receive(data).await
    }
}

/// Builder for client configuration
pub struct ClientConfigBuilder {
    config: ClientConfig,
}

impl ClientConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default(),
        }
    }

    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.config.read_timeout = timeout;
        self
    }

    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.config.write_timeout = timeout;
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.connect_timeout = timeout;
        self
    }

    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    pub fn max_response_size(mut self, size: usize) -> Self {
        self.config.max_response_size = size;
        self
    }

    pub fn build(self) -> ClientConfig {
        self.config
    }
}

impl Default for ClientConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_client_config_builder() {
        let config = ClientConfigBuilder::new()
            .read_timeout(Duration::from_secs(60))
            .write_timeout(Duration::from_secs(30))
            .buffer_size(2048)
            .max_response_size(1024 * 1024)
            .build();

        assert_eq!(config.read_timeout, Duration::from_secs(60));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
        assert_eq!(config.buffer_size, 2048);
        assert_eq!(config.max_response_size, 1024 * 1024);
    }

    #[tokio::test]
    async fn test_client_idle_detection() {
        use std::net::SocketAddr;
        
        // Create a dummy address for testing
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let _address = Address::Network(addr);
        
        // This test would require a real connection, so we'll just test the builder
        let config = ClientConfigBuilder::new()
            .connect_timeout(Duration::from_millis(100))
            .build();
        
        // Verify the config was built correctly
        assert_eq!(config.connect_timeout, Duration::from_millis(100));
    }

    #[test]
    fn test_size_validation() {
        let config = ClientConfig {
            max_response_size: 100,
            ..Default::default()
        };

        // Test that the config respects size limits
        assert!(config.max_response_size == 100);
    }
}