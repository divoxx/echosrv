use crate::network::{BindStrategy, BindTarget};
use crate::{EchoError, Result};
use std::net::SocketAddr;
use std::time::Duration;

/// Default datagram buffer size: 64 KiB, large enough for any UDP payload over IPv4.
pub const DEFAULT_DATAGRAM_BUFFER_SIZE: usize = 64 * 1024;

/// Configuration for datagram-based echo servers
///
/// This struct contains all the configuration options needed for
/// datagram-based echo servers (UDP, Unix datagrams, etc.).
///
/// # Examples
///
/// ```
/// use echosrv::datagram::DatagramConfig;
/// use std::time::Duration;
///
/// let config = DatagramConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     ..Default::default()
/// };
/// assert!(config.validate().is_ok());
/// ```
#[derive(Debug, Clone)]
pub struct DatagramConfig {
    /// Network address to bind the server to.
    ///
    /// Used when [`bind_strategy`](Self::bind_strategy) is `None`; ignored otherwise.
    pub bind_addr: SocketAddr,
    /// Receive buffer size; datagrams larger than this are truncated (must be non-zero)
    pub buffer_size: usize,
    /// Idle receive timeout. Expiry is not an error; the server keeps waiting.
    pub read_timeout: Duration,
    /// Timeout for sending each echo reply
    pub write_timeout: Duration,
    /// How to obtain the socket. `None` (the default) binds
    /// [`bind_addr`](Self::bind_addr); `Some` overrides it (inheritance, Unix path).
    pub bind_strategy: Option<BindStrategy>,
    /// Service name used to look up an inherited descriptor when the strategy
    /// is [`BindStrategy::InheritOrBind`] without an explicit descriptor.
    pub service_name: String,
}

impl Default for DatagramConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            buffer_size: DEFAULT_DATAGRAM_BUFFER_SIZE,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            bind_strategy: None,
            service_name: "datagram".to_string(),
        }
    }
}

impl DatagramConfig {
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

    /// Checks the configuration; returns [`EchoError::Config`] if `buffer_size` is zero.
    pub fn validate(&self) -> Result<()> {
        if self.buffer_size == 0 {
            return Err(EchoError::Config(
                "buffer_size must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

/// Configuration for datagram echo clients.
///
/// # Examples
///
/// ```
/// use echosrv::datagram::DatagramClientConfig;
/// use std::time::Duration;
///
/// let config = DatagramClientConfig {
///     read_timeout: Duration::from_secs(1),
///     ..Default::default()
/// };
/// assert_eq!(config.buffer_size, 64 * 1024);
/// ```
#[derive(Debug, Clone)]
pub struct DatagramClientConfig {
    /// Receive buffer size; replies larger than this are truncated
    pub buffer_size: usize,
    /// How long to wait for the echoed reply
    pub read_timeout: Duration,
    /// How long to wait for the send to complete
    pub write_timeout: Duration,
}

impl Default for DatagramClientConfig {
    fn default() -> Self {
        Self {
            buffer_size: DEFAULT_DATAGRAM_BUFFER_SIZE,
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
        }
    }
}
