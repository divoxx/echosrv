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
    /// Used when [`bind_strategy`](Self::bind_strategy) is `None`, and as the
    /// fallback of an [`InheritOrBind`](BindStrategy::InheritOrBind) strategy
    /// without its own fallback target (see
    /// [`with_fd_inheritance`](Self::with_fd_inheritance)).
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
    use crate::network::InheritedFd;

    #[test]
    fn defaults() {
        let config = StreamConfig::default();
        assert_eq!(config.bind_addr, "127.0.0.1:0".parse().unwrap());
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.buffer_size, 1024);
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
        assert!(config.bind_strategy.is_none());
        assert_eq!(config.service_name, "stream");
        config.validate().unwrap();
    }

    #[test]
    fn rejects_zero_buffer() {
        let config = StreamConfig {
            buffer_size: 0,
            ..Default::default()
        };
        match config.validate() {
            Err(EchoError::Config(msg)) => assert!(msg.contains("buffer_size")),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_zero_max_connections() {
        let config = StreamConfig {
            max_connections: 0,
            ..Default::default()
        };
        match config.validate() {
            Err(EchoError::Config(msg)) => assert!(msg.contains("max_connections")),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn accepts_minimal_values() {
        let config = StreamConfig {
            buffer_size: 1,
            max_connections: 1,
            read_timeout: Duration::ZERO,
            write_timeout: Duration::ZERO,
            ..Default::default()
        };
        config.validate().unwrap();
    }

    #[test]
    fn effective_strategy_defaults_to_bind_addr() {
        let config = StreamConfig {
            bind_addr: "10.1.2.3:4567".parse().unwrap(),
            ..Default::default()
        };
        match config.effective_bind_strategy() {
            BindStrategy::Bind(BindTarget::Network(addr)) => assert_eq!(addr, config.bind_addr),
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn effective_strategy_prefers_explicit_strategy() {
        let config = StreamConfig {
            bind_strategy: Some(BindStrategy::Bind(BindTarget::Unix("/tmp/s.sock".into()))),
            ..Default::default()
        };
        match config.effective_bind_strategy() {
            BindStrategy::Bind(BindTarget::Unix(path)) => {
                assert_eq!(path.as_os_str(), "/tmp/s.sock")
            }
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn effective_strategy_shares_inherited_fd() {
        let fd = InheritedFd::new(std::net::TcpListener::bind("127.0.0.1:0").unwrap().into());
        let config = StreamConfig {
            bind_strategy: Some(BindStrategy::Inherit(fd.clone())),
            ..Default::default()
        };
        // The cloned strategy refers to the same take-once descriptor.
        match config.effective_bind_strategy() {
            BindStrategy::Inherit(inner) => assert!(inner.take().is_some()),
            other => panic!("unexpected strategy {other:?}"),
        }
        assert!(fd.is_consumed());
    }

    #[test]
    fn with_fd_inheritance_falls_back_to_bind_addr() {
        let base = StreamConfig {
            bind_addr: "0.0.0.0:8080".parse().unwrap(),
            max_connections: 7,
            ..Default::default()
        };
        let config = base.with_fd_inheritance("web");
        assert_eq!(config.service_name, "web");
        assert_eq!(config.max_connections, 7);
        match config.effective_bind_strategy() {
            BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: Some(BindTarget::Network(addr)),
            } => assert_eq!(addr, "0.0.0.0:8080".parse().unwrap()),
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn with_fd_inheritance_uses_bind_addr_set_afterwards() {
        let mut config = StreamConfig::default().with_fd_inheritance("svc");
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
        let config = StreamConfig {
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
