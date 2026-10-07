//! UDP server configuration.

use crate::datagram::{DEFAULT_DATAGRAM_BUFFER_SIZE, DatagramConfig};
use crate::network::BindStrategy;
use std::net::SocketAddr;
use std::time::Duration;

/// Configuration for [`UdpEchoServer`](crate::UdpEchoServer).
///
/// Converts into [`DatagramConfig`] (field for field), which is what
/// `UdpEchoServer::new` takes. Defaults: `127.0.0.1:0`, 64 KiB buffer, 30 s
/// timeouts, service name `"udp"`.
///
/// # Examples
///
/// ```
/// use echosrv::udp::UdpConfig;
/// use std::time::Duration;
///
/// let config = UdpConfig {
///     bind_addr: "127.0.0.1:8080".parse().unwrap(),
///     buffer_size: 1024,
///     read_timeout: Duration::from_secs(30),
///     write_timeout: Duration::from_secs(30),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UdpConfig {
    /// Address to bind the server to (when not inheriting).
    pub bind_addr: SocketAddr,
    /// Receive buffer size (default 64 KiB); larger datagrams are truncated
    pub buffer_size: usize,
    /// Idle receive timeout. Expiry is not an error; the server keeps waiting.
    pub read_timeout: Duration,
    /// Timeout for sending each reply.
    pub write_timeout: Duration,
    /// Socket acquisition strategy; `None` binds `bind_addr`.
    /// See [`DatagramConfig::bind_strategy`].
    pub bind_strategy: Option<BindStrategy>,
    /// Service name for inherited-descriptor lookup (default `"udp"`).
    pub service_name: String,
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            buffer_size: DEFAULT_DATAGRAM_BUFFER_SIZE,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            bind_strategy: None,
            service_name: "udp".to_string(),
        }
    }
}

impl UdpConfig {
    /// Prefer an inherited descriptor named `service_name` (e.g. systemd
    /// `FileDescriptorName=`), or the only descriptor if exactly one was
    /// passed, falling back to binding `bind_addr`.
    ///
    /// Sets [`bind_strategy`](Self::bind_strategy) to
    /// [`BindStrategy::InheritOrBind`]; see [`take_named_or_sole`] for the
    /// lookup rule.
    ///
    /// [`take_named_or_sole`]: crate::network::FdInheritanceConfig::take_named_or_sole
    ///
    /// The fallback address is read when the server binds, so `bind_addr`
    /// may still be changed afterwards.
    pub fn with_fd_inheritance(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self.bind_strategy = Some(BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: None,
        });
        self
    }
}

impl From<UdpConfig> for DatagramConfig {
    fn from(config: UdpConfig) -> Self {
        Self {
            bind_addr: config.bind_addr,
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            bind_strategy: config.bind_strategy,
            service_name: config.service_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BindTarget;

    #[test]
    fn from_preserves_every_field() {
        let config = UdpConfig {
            bind_addr: "[::]:5300".parse().unwrap(),
            buffer_size: 99,
            read_timeout: Duration::from_millis(11),
            write_timeout: Duration::from_millis(22),
            bind_strategy: Some(BindStrategy::Bind(BindTarget::Network(
                "127.0.0.1:5301".parse().unwrap(),
            ))),
            service_name: "dns".into(),
        };
        let dgram: DatagramConfig = config.into();
        assert_eq!(dgram.bind_addr, "[::]:5300".parse().unwrap());
        assert_eq!(dgram.buffer_size, 99);
        assert_eq!(dgram.read_timeout, Duration::from_millis(11));
        assert_eq!(dgram.write_timeout, Duration::from_millis(22));
        assert_eq!(dgram.service_name, "dns");
        match dgram.bind_strategy {
            Some(BindStrategy::Bind(BindTarget::Network(addr))) => {
                assert_eq!(addr, "127.0.0.1:5301".parse().unwrap())
            }
            other => panic!("strategy not preserved: {other:?}"),
        }
    }

    #[test]
    fn default_converts_to_valid_datagram_config() {
        let config = UdpConfig::default();
        assert_eq!(config.bind_addr, "127.0.0.1:0".parse().unwrap());
        assert!(config.bind_strategy.is_none());
        let dgram: DatagramConfig = config.into();
        assert_eq!(dgram.service_name, "udp");
        dgram.validate().unwrap();
    }

    #[test]
    fn with_fd_inheritance_falls_back_to_current_bind_addr() {
        let mut config = UdpConfig::default().with_fd_inheritance("svc");
        // Changing bind_addr after enabling inheritance must take effect.
        config.bind_addr = "127.0.0.1:4321".parse().unwrap();
        let converted: DatagramConfig = config.into();
        match converted.effective_bind_strategy() {
            BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: Some(BindTarget::Network(addr)),
            } => assert_eq!(addr, "127.0.0.1:4321".parse().unwrap()),
            other => panic!("unexpected strategy {other:?}"),
        }
    }

    #[test]
    fn with_fd_inheritance() {
        let config = UdpConfig {
            bind_addr: "0.0.0.0:9090".parse().unwrap(),
            ..Default::default()
        }
        .with_fd_inheritance("echo-udp");
        assert_eq!(config.service_name, "echo-udp");
        match &config.bind_strategy {
            Some(BindStrategy::InheritOrBind {
                fd: None,
                fallback_target: None,
            }) => {}
            other => panic!("unexpected strategy {other:?}"),
        }
    }
}
