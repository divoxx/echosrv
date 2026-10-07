use crate::network::{BindStrategy, BindTarget};
use crate::{EchoError, Result};
use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for stream-based echo servers
///
/// This struct contains all the configuration options needed for
/// stream-based echo servers (TCP, HTTP, Unix streams).
///
/// # Examples
///
/// ```
/// use echosrv::stream::StreamConfig;
/// use std::time::Duration;
///
/// let config = StreamConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     max_connections: 100,
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     ..Default::default()
/// };
/// assert!(config.validate().is_ok());
/// ```
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Network address to bind the server to.
    ///
    /// Used when [`bind_strategy`](Self::bind_strategy) is `None`; ignored otherwise.
    pub bind_addr: SocketAddr,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Buffer size for reading/writing data (must be non-zero)
    pub buffer_size: usize,
    /// Read timeout for connections
    pub read_timeout: Duration,
    /// Write timeout for connections
    pub write_timeout: Duration,
    /// How to obtain the listening socket.
    ///
    /// `None` (the default) binds [`bind_addr`](Self::bind_addr). `Some`
    /// overrides it, e.g. to inherit a descriptor from systemd or to bind a
    /// Unix socket path.
    pub bind_strategy: Option<BindStrategy>,
    /// Service name used to look up an inherited descriptor (e.g. a systemd
    /// `FileDescriptorName=`) when the strategy is
    /// [`BindStrategy::InheritOrBind`] without an explicit descriptor.
    pub service_name: String,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            max_connections: 100,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            bind_strategy: None,
            service_name: "stream".to_string(),
        }
    }
}

impl StreamConfig {
    /// The effective bind strategy: [`bind_strategy`](Self::bind_strategy) if
    /// set, otherwise binding [`bind_addr`](Self::bind_addr).
    pub fn effective_bind_strategy(&self) -> BindStrategy {
        self.bind_strategy
            .clone()
            .unwrap_or(BindStrategy::Bind(BindTarget::Network(self.bind_addr)))
    }

    /// Prefer an inherited descriptor named `service_name`, falling back to
    /// binding [`bind_addr`](Self::bind_addr).
    pub fn with_fd_inheritance(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self.bind_strategy = Some(BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: BindTarget::Network(self.bind_addr),
        });
        self
    }

    /// Checks the configuration for values that cannot work.
    ///
    /// Returns [`EchoError::Config`] if `buffer_size` or `max_connections` is zero.
    pub fn validate(&self) -> Result<()> {
        if self.buffer_size == 0 {
            return Err(EchoError::Config(
                "buffer_size must be greater than 0".into(),
            ));
        }
        if self.max_connections == 0 {
            return Err(EchoError::Config(
                "max_connections must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_buffer() {
        let config = StreamConfig {
            buffer_size: 0,
            ..Default::default()
        };
        assert!(matches!(config.validate(), Err(EchoError::Config(_))));
    }

    #[test]
    fn effective_strategy_defaults_to_bind_addr() {
        let config = StreamConfig::default();
        match config.effective_bind_strategy() {
            BindStrategy::Bind(BindTarget::Network(addr)) => assert_eq!(addr, config.bind_addr),
            other => panic!("unexpected strategy {other:?}"),
        }
    }
}
