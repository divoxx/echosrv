//! Configuration for datagram echo servers and clients.

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
    /// Used when [`bind_strategy`](Self::bind_strategy) is `None`, and as the
    /// fallback of an [`InheritOrBind`](BindStrategy::InheritOrBind) strategy
    /// without its own fallback target (see
    /// [`with_fd_inheritance`](Self::with_fd_inheritance)).
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
    ///
    /// An [`InheritOrBind`](BindStrategy::InheritOrBind) strategy without a
    /// fallback target falls back to the *current* `bind_addr`.
    pub fn effective_bind_strategy(&self) -> BindStrategy {
        let bind_addr = BindTarget::Network(self.bind_addr);
        match &self.bind_strategy {
            Some(strategy) => strategy.clone().with_default_fallback(bind_addr),
            None => BindStrategy::Bind(bind_addr),
        }
    }

    /// Prefer an inherited descriptor named `service_name` (or the only one
    /// passed), falling back to binding [`bind_addr`](Self::bind_addr).
    ///
    /// The fallback address is read when the socket is bound, so `bind_addr`
    /// may still be changed afterwards.
    pub fn with_fd_inheritance(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self.bind_strategy = Some(BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let config = DatagramConfig::default();
        assert_eq!(config.bind_addr, "127.0.0.1:0".parse().unwrap());
        assert_eq!(config.buffer_size, 64 * 1024);
        assert_eq!(config.buffer_size, DEFAULT_DATAGRAM_BUFFER_SIZE);
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
        assert!(config.bind_strategy.is_none());
        assert_eq!(config.service_name, "datagram");
        config.validate().unwrap();
    }

    #[test]
    fn client_defaults() {
        let config = DatagramClientConfig::default();
        assert_eq!(config.buffer_size, DEFAULT_DATAGRAM_BUFFER_SIZE);
        assert_eq!(config.read_timeout, Duration::from_secs(5));
        assert_eq!(config.write_timeout, Duration::from_secs(5));
    }

    #[test]
    fn rejects_zero_buffer() {
        let config = DatagramConfig {
            buffer_size: 0,
            ..Default::default()
        };
        match config.validate() {
            Err(EchoError::Config(msg)) => assert!(msg.contains("buffer_size")),
            other => panic!("expected Config error, got {other:?}"),
        }
        DatagramConfig {
            buffer_size: 1,
            ..Default::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn effective_strategy() {
        let config = DatagramConfig {
            bind_addr: "[::1]:5353".parse().unwrap(),
            ..Default::default()
        };
        match config.effective_bind_strategy() {
            BindStrategy::Bind(BindTarget::Network(addr)) => assert_eq!(addr, config.bind_addr),
            other => panic!("unexpected strategy {other:?}"),
        }

        let config = DatagramConfig {
            bind_strategy: Some(BindStrategy::Bind(BindTarget::Unix("/tmp/d.sock".into()))),
            ..config
        };
        assert!(matches!(
            config.effective_bind_strategy(),
            BindStrategy::Bind(BindTarget::Unix(_))
        ));
    }

    #[test]
    fn with_fd_inheritance_falls_back_to_bind_addr() {
        let config = DatagramConfig {
            bind_addr: "0.0.0.0:9999".parse().unwrap(),
            buffer_size: 512,
            ..Default::default()
        }
        .with_fd_inheritance("dns");
        assert_eq!(config.service_name, "dns");
        assert_eq!(config.buffer_size, 512);
        match config.effective_bind_strategy() {
            BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: Some(BindTarget::Network(addr)),
            } => assert_eq!(addr, "0.0.0.0:9999".parse().unwrap()),
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn with_fd_inheritance_uses_bind_addr_set_afterwards() {
        let mut config = DatagramConfig::default().with_fd_inheritance("svc");
        config.bind_addr = "127.0.0.1:4321".parse().unwrap();
        // The fallback is not snapshotted when inheritance is enabled.
        assert!(matches!(
            config.bind_strategy,
            Some(BindStrategy::InheritOrBind {
                fallback_target: None,
                ..
            })
        ));
        match config.effective_bind_strategy() {
            BindStrategy::InheritOrBind {
                fallback_target: Some(BindTarget::Network(addr)),
                ..
            } => assert_eq!(addr, config.bind_addr),
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn effective_strategy_keeps_explicit_fallback() {
        let config = DatagramConfig {
            bind_strategy: Some(BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: Some(BindTarget::Network("127.0.0.1:1".parse().unwrap())),
            }),
            bind_addr: "127.0.0.1:2".parse().unwrap(),
            ..Default::default()
        };
        match config.effective_bind_strategy() {
            BindStrategy::InheritOrBind {
                fallback_target: Some(BindTarget::Network(addr)),
                ..
            } => assert_eq!(addr.port(), 1),
            other => panic!("unexpected strategy {other:?}"),
        }
    }
}
