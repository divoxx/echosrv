use super::Address;
use std::time::Duration;

/// Universal configuration for echo servers
#[derive(Debug, Clone)]
pub struct Config<A = Address> {
    /// Address to bind the server to
    pub bind_addr: A,
    /// Buffer size for reading/writing data
    pub buffer_size: usize,
    /// Read timeout for connections/datagrams
    pub read_timeout: Duration,
    /// Write timeout for connections/datagrams
    pub write_timeout: Duration,
}

impl<A> Config<A> {
    /// Create a new configuration with the given address
    pub fn new(bind_addr: A) -> Self {
        Self {
            bind_addr,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }

    /// Set the buffer size
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Set the read timeout
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Set the write timeout
    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }
}

/// Configuration specific to stream protocols (adds connection limits)
#[derive(Debug, Clone)]
pub struct StreamConfig<A = Address> {
    /// Base configuration
    pub base: Config<A>,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
}

impl<A> StreamConfig<A> {
    /// Create a new stream configuration with the given address
    pub fn new(bind_addr: A) -> Self {
        Self {
            base: Config::new(bind_addr),
            max_connections: 100,
        }
    }

    /// Set the maximum number of concurrent connections
    pub fn with_max_connections(mut self, max_connections: usize) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// Set the buffer size
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.base.buffer_size = buffer_size;
        self
    }

    /// Set the read timeout
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.base.read_timeout = timeout;
        self
    }

    /// Set the write timeout
    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.base.write_timeout = timeout;
        self
    }
}

impl Default for Config<Address> {
    fn default() -> Self {
        Self::new("127.0.0.1:0".into())
    }
}

impl Default for StreamConfig<Address> {
    fn default() -> Self {
        Self::new("127.0.0.1:0".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config: Config<Address> = Config::new("127.0.0.1:8080".into())
            .with_buffer_size(2048)
            .with_read_timeout(Duration::from_secs(60));

        assert_eq!(config.buffer_size, 2048);
        assert_eq!(config.read_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_stream_config_builder() {
        let config: StreamConfig<Address> = StreamConfig::new("127.0.0.1:8080".into())
            .with_max_connections(200)
            .with_buffer_size(4096);

        assert_eq!(config.max_connections, 200);
        assert_eq!(config.base.buffer_size, 4096);
    }
}
