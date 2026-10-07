//! Generic socket building infrastructure for FD inheritance.
//!
//! Socket creation follows a common pattern for every protocol:
//! 1. Resolve the [`BindStrategy`] into a concrete [`SocketSource`]
//!    (an inherited, owned descriptor or a target to bind).
//! 2. Validate inherited descriptors match the expected socket type/family.
//! 3. Convert the descriptor (or freshly bound socket) into a Tokio type.
//!
//! Protocol-specific details live in [`BuildSocket`] implementations
//! (TCP, UDP, Unix stream, Unix datagram).

use crate::network::fd_inheritance::{BindStrategy, FdInheritanceConfig, validation};

// Re-export types that builders need
pub use crate::network::fd_inheritance::BindTarget;
use crate::{EchoError, Result};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

/// Generic socket builder that handles FD inheritance logic
///
/// The type parameter `T` represents the final socket type (`TcpListener`, `UdpSocket`, etc.)
pub struct SocketBuilder<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> SocketBuilder<T> {
    /// Resolve a binding strategy into a concrete socket source.
    ///
    /// * [`BindStrategy::Bind`] → bind the target.
    /// * [`BindStrategy::Inherit`] → take the descriptor; error if it was
    ///   already consumed (e.g. by a previous `run()`).
    /// * [`BindStrategy::InheritOrBind`] → take the explicit descriptor if still
    ///   available, else take one from `fd_config` with
    ///   [`FdInheritanceConfig::take_named_or_sole`] (the descriptor named
    ///   `service_name`, else the only descriptor in the pool), else bind the
    ///   fallback target. An unset fallback target (not resolved by the server
    ///   config) is an [`EchoError::Config`] error.
    ///
    /// Inherited descriptors are *moved out* of the strategy / pool, so each
    /// descriptor is owned by exactly one socket.
    pub fn resolve_fd(
        strategy: &BindStrategy,
        service_name: &str,
        fd_config: &FdInheritanceConfig,
    ) -> Result<SocketSource> {
        match strategy {
            BindStrategy::Bind(target) => Ok(SocketSource::Bind(target.clone())),
            BindStrategy::Inherit(fd) => fd.take().map(SocketSource::Inherit).ok_or_else(|| {
                EchoError::FdInheritance(format!(
                    "Inherited fd {} was already consumed by another socket",
                    fd.raw_fd()
                ))
            }),
            BindStrategy::InheritOrBind {
                fd,
                fallback_target,
            } => {
                if let Some(owned) = fd.as_ref().and_then(|fd| fd.take()) {
                    return Ok(SocketSource::Inherit(owned));
                }
                if let Some(owned) = fd_config.take_named_or_sole(service_name) {
                    tracing::info!(
                        service = service_name,
                        "Using socket inherited from parent process"
                    );
                    return Ok(SocketSource::Inherit(owned));
                }
                if fd_config.has_inherited_fds() {
                    tracing::warn!(
                        service = service_name,
                        available = ?fd_config.inherited_service_names(),
                        "No matching inherited socket; binding a new one"
                    );
                }
                fallback_target
                    .clone()
                    .map(SocketSource::Bind)
                    .ok_or_else(|| {
                        EchoError::Config(format!(
                            "No inherited socket for service '{service_name}' and no fallback \
                             target to bind"
                        ))
                    })
            }
        }
    }
}

/// Resolved socket source after inheritance logic is applied
#[derive(Debug)]
pub enum SocketSource {
    /// Create socket by binding to specified target
    Bind(BindTarget),
    /// Create socket from an inherited, owned file descriptor
    Inherit(OwnedFd),
}

/// Trait for protocol-specific socket building
///
/// Each socket type (TCP, UDP, Unix stream, Unix datagram) implements this trait
/// to provide protocol-specific creation logic while sharing the common inheritance
/// framework provided by [`SocketBuilder`].
pub trait BuildSocket<T> {
    /// Expected socket type for validation (`SOCK_STREAM`, `SOCK_DGRAM`)
    const SOCKET_TYPE: libc::c_int;

    /// Valid address families for this socket type
    ///
    /// Network sockets accept `AF_INET` and `AF_INET6`, Unix domain sockets
    /// only `AF_UNIX`.
    const VALID_FAMILIES: &'static [libc::c_int];

    /// Whether inherited descriptors must already be listening (`SO_ACCEPTCONN`).
    /// `true` for stream listeners, `false` for datagram sockets.
    const REQUIRE_LISTENING: bool = false;

    /// Create socket from an inherited, already validated file descriptor.
    fn from_fd(fd: OwnedFd) -> Result<T>;

    /// Create socket by binding to specified target
    ///
    /// Must reject targets of the wrong kind (e.g. TCP cannot bind a Unix path).
    fn bind_to(target: &BindTarget) -> Result<T>;

    /// Main entry point for socket creation
    ///
    /// 1. Resolve the strategy to a concrete source (taking ownership of any
    ///    inherited descriptor).
    /// 2. Validate inherited descriptors.
    /// 3. Create the socket.
    fn build(
        strategy: &BindStrategy,
        service_name: &str,
        fd_config: &FdInheritanceConfig,
    ) -> Result<T> {
        match SocketBuilder::<T>::resolve_fd(strategy, service_name, fd_config)? {
            SocketSource::Inherit(fd) => {
                Self::validate_inherited_fd(fd.as_fd())?;
                Self::from_fd(fd)
            }
            SocketSource::Bind(target) => Self::bind_to(&target),
        }
    }

    /// Validate an inherited file descriptor matches socket requirements:
    /// socket type, address family and (for listeners) listening state.
    fn validate_inherited_fd(fd: BorrowedFd<'_>) -> Result<()> {
        validation::validate_socket_type(fd, Self::SOCKET_TYPE)?;

        let mut last_error = None;
        let mut family_ok = false;
        for &family in Self::VALID_FAMILIES {
            match validation::validate_socket_family(fd, family) {
                Ok(()) => {
                    family_ok = true;
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        if !family_ok {
            return Err(last_error.unwrap_or_else(|| {
                EchoError::FdInheritance(
                    "No valid socket families configured for this protocol".into(),
                )
            }));
        }

        if Self::REQUIRE_LISTENING {
            validation::validate_listening(fd)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::InheritedFd;
    use crate::tcp::socket_builder::TcpSocketBuilder;
    use crate::udp::socket_builder::UdpSocketBuilder;
    use crate::unix::datagram_protocol::UnixDatagramSocketBuilder;
    use crate::unix::stream_protocol::UnixStreamSocketBuilder;
    use std::net::SocketAddr;
    use std::os::fd::AsRawFd;

    /// `SocketBuilder<T>`'s type parameter does not affect resolution.
    type Resolver = SocketBuilder<()>;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    fn tcp_listener_fd() -> (OwnedFd, SocketAddr) {
        let listener = std::net::TcpListener::bind(loopback()).unwrap();
        let addr = listener.local_addr().unwrap();
        (listener.into(), addr)
    }

    #[track_caller]
    fn expect_bind(source: SocketSource) -> BindTarget {
        match source {
            SocketSource::Bind(target) => target,
            SocketSource::Inherit(fd) => panic!("expected Bind, got Inherit({fd:?})"),
        }
    }

    #[track_caller]
    fn expect_inherit(source: SocketSource) -> OwnedFd {
        match source {
            SocketSource::Inherit(fd) => fd,
            SocketSource::Bind(target) => panic!("expected Inherit, got Bind({target:?})"),
        }
    }

    // --- resolve_fd ----------------------------------------------------------

    #[test]
    fn resolve_bind_returns_target_and_ignores_pool() {
        let (fd, _) = tcp_listener_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), fd)]);
        let strategy = BindStrategy::Bind(BindTarget::Network(loopback()));

        let target = expect_bind(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert!(matches!(target, BindTarget::Network(a) if a == loopback()));
        assert!(pool.has_inherited_fds(), "Bind must not touch the pool");
    }

    #[test]
    fn resolve_inherit_takes_fd_once() {
        let (fd, _) = tcp_listener_fd();
        let raw = fd.as_raw_fd();
        let inherited = InheritedFd::new(fd);
        let strategy = BindStrategy::Inherit(inherited.clone());
        let pool = FdInheritanceConfig::empty();

        let owned = expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert_eq!(owned.as_raw_fd(), raw);
        assert!(inherited.is_consumed());

        // Resolving again (e.g. a second run()) must not double-own the fd.
        match Resolver::resolve_fd(&strategy, "svc", &pool) {
            Err(EchoError::FdInheritance(msg)) => assert!(msg.contains("already consumed")),
            other => panic!("expected FdInheritance error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_inherit_ignores_pool_even_if_name_matches() {
        let (consumed_fd, _) = tcp_listener_fd();
        let inherited = InheritedFd::new(consumed_fd);
        drop(inherited.take());
        let (pool_fd, _) = tcp_listener_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), pool_fd)]);

        let strategy = BindStrategy::Inherit(inherited);
        assert!(Resolver::resolve_fd(&strategy, "svc", &pool).is_err());
        assert!(pool.has_inherited_fds());
    }

    #[test]
    fn resolve_inherit_or_bind_prefers_explicit_fd() {
        let (explicit, _) = tcp_listener_fd();
        let raw = explicit.as_raw_fd();
        let (pooled, _) = tcp_listener_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), pooled)]);
        let strategy = BindStrategy::InheritOrBind {
            fd: Some(InheritedFd::new(explicit)),
            fallback_target: Some(BindTarget::Network(loopback())),
        };

        let owned = expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert_eq!(owned.as_raw_fd(), raw);
        assert!(pool.has_inherited_fds(), "pool used despite explicit fd");
    }

    #[test]
    fn resolve_inherit_or_bind_consumed_fd_falls_back_to_pool_then_bind() {
        let (explicit, _) = tcp_listener_fd();
        let (pooled, _) = tcp_listener_fd();
        let pooled_raw = pooled.as_raw_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), pooled)]);
        let strategy = BindStrategy::InheritOrBind {
            fd: Some(InheritedFd::new(explicit)),
            fallback_target: Some(BindTarget::Network(loopback())),
        };

        // 1st: explicit fd, 2nd: pool by name, 3rd: fallback bind.
        expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        let owned = expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert_eq!(owned.as_raw_fd(), pooled_raw);
        expect_bind(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
    }

    #[test]
    fn resolve_inherit_or_bind_without_fd_uses_pool_by_name() {
        let (pooled, _) = tcp_listener_fd();
        let raw = pooled.as_raw_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), pooled)]);
        let strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix("/unused.sock".into())),
        };

        let owned = expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert_eq!(owned.as_raw_fd(), raw);
        assert!(!pool.has_inherited_fds());
    }

    #[test]
    fn resolve_inherit_or_bind_without_name_match_takes_sole_fd() {
        // Same rule as the CLI: a single descriptor is used whatever its name
        // (systemd names unnamed sockets "unknown").
        let (pooled, _) = tcp_listener_fd();
        let raw = pooled.as_raw_fd();
        let pool = FdInheritanceConfig::from_fds([(
            crate::network::fd_inheritance::SYSTEMD_UNNAMED_FD.to_string(),
            pooled,
        )]);
        let strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix("/fallback.sock".into())),
        };

        let owned = expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert_eq!(owned.as_raw_fd(), raw);
        assert!(!pool.has_inherited_fds());
    }

    #[test]
    fn resolve_inherit_or_bind_without_match_binds_fallback() {
        let (a, _) = tcp_listener_fd();
        let (b, _) = tcp_listener_fd();
        let pool = FdInheritanceConfig::from_fds([("a".to_string(), a), ("b".to_string(), b)]);
        let strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix("/fallback.sock".into())),
        };

        // Several descriptors, none named "svc": ambiguous, so bind.
        let target = expect_bind(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
        assert!(matches!(target, BindTarget::Unix(p) if p.as_os_str() == "/fallback.sock"));
        assert_eq!(pool.inherited_service_names().len(), 2);

        let empty = FdInheritanceConfig::empty();
        expect_bind(Resolver::resolve_fd(&strategy, "svc", &empty).unwrap());
    }

    #[test]
    fn resolve_inherit_or_bind_without_fallback_is_config_error() {
        let strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: None,
        };
        match Resolver::resolve_fd(&strategy, "svc", &FdInheritanceConfig::empty()) {
            Err(EchoError::Config(msg)) => assert!(msg.contains("svc"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }

        // A pooled descriptor still wins.
        let (pooled, _) = tcp_listener_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), pooled)]);
        expect_inherit(Resolver::resolve_fd(&strategy, "svc", &pool).unwrap());
    }

    // --- BuildSocket::build ----------------------------------------------------

    #[track_caller]
    fn assert_fd_inheritance_err<T: std::fmt::Debug>(result: Result<T>, needle: &str) {
        match result {
            Err(EchoError::FdInheritance(msg)) => {
                assert!(msg.contains(needle), "{msg:?} does not mention {needle:?}")
            }
            other => panic!("expected FdInheritance error, got {other:?}"),
        }
    }

    fn inherit(fd: impl Into<OwnedFd>) -> BindStrategy {
        BindStrategy::Inherit(InheritedFd::new(fd.into()))
    }

    #[tokio::test]
    async fn build_tcp_from_inherited_listener() {
        let (fd, addr) = tcp_listener_fd();
        let listener =
            TcpSocketBuilder::build(&inherit(fd), "tcp", &FdInheritanceConfig::empty()).unwrap();
        assert_eq!(listener.local_addr().unwrap(), addr);
    }

    #[tokio::test]
    async fn build_tcp_from_pool_by_service_name() {
        let (fd, addr) = tcp_listener_fd();
        let pool = FdInheritanceConfig::from_fds([("tcp".to_string(), fd)]);
        let strategy = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Network(loopback())),
        };
        let listener = TcpSocketBuilder::build(&strategy, "tcp", &pool).unwrap();
        assert_eq!(listener.local_addr().unwrap(), addr);
    }

    #[tokio::test]
    async fn build_tcp_rejects_wrong_fds() {
        let pool = FdInheritanceConfig::empty();

        let udp = std::net::UdpSocket::bind(loopback()).unwrap();
        assert_fd_inheritance_err(
            TcpSocketBuilder::build(&inherit(udp), "tcp", &pool),
            "SOCK_STREAM",
        );

        let dir = tempfile::tempdir().unwrap();
        let unix = std::os::unix::net::UnixListener::bind(dir.path().join("s.sock")).unwrap();
        assert_fd_inheritance_err(
            TcpSocketBuilder::build(&inherit(unix), "tcp", &pool),
            "AF_UNIX",
        );

        let file = tempfile::tempfile().unwrap();
        assert_fd_inheritance_err(
            TcpSocketBuilder::build(&inherit(file), "tcp", &pool),
            "socket type",
        );
    }

    #[tokio::test]
    async fn build_tcp_rejects_connected_non_listening_socket() {
        let listener = std::net::TcpListener::bind(loopback()).unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let result =
            TcpSocketBuilder::build(&inherit(stream), "tcp", &FdInheritanceConfig::empty());
        // Platforms without SO_ACCEPTCONN support skip the listening check.
        match result {
            Err(EchoError::FdInheritance(msg)) => assert!(msg.contains("listening"), "{msg}"),
            Ok(_) => eprintln!("SO_ACCEPTCONN unsupported on this platform; check skipped"),
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }

    #[tokio::test]
    async fn build_rejects_wrong_bind_target_kind() {
        let pool = FdInheritanceConfig::empty();
        let unix = BindStrategy::Bind(BindTarget::Unix("/tmp/x.sock".into()));
        let network = BindStrategy::Bind(BindTarget::Network(loopback()));

        assert!(matches!(
            TcpSocketBuilder::build(&unix, "tcp", &pool),
            Err(EchoError::Config(_))
        ));
        assert!(matches!(
            UdpSocketBuilder::build(&unix, "udp", &pool),
            Err(EchoError::Config(_))
        ));
        assert!(matches!(
            UnixStreamSocketBuilder::build(&network, "unix", &pool),
            Err(EchoError::Config(_))
        ));
        assert!(matches!(
            UnixDatagramSocketBuilder::build(&network, "unix", &pool),
            Err(EchoError::Config(_))
        ));
    }

    #[tokio::test]
    async fn build_udp_from_inherited_socket_and_rejects_tcp() {
        let pool = FdInheritanceConfig::empty();
        let udp = std::net::UdpSocket::bind(loopback()).unwrap();
        let addr = udp.local_addr().unwrap();
        let socket = UdpSocketBuilder::build(&inherit(udp), "udp", &pool).unwrap();
        assert_eq!(socket.local_addr().unwrap(), addr);

        let (tcp, _) = tcp_listener_fd();
        assert_fd_inheritance_err(
            UdpSocketBuilder::build(&inherit(tcp), "udp", &pool),
            "SOCK_DGRAM",
        );
    }

    #[tokio::test]
    async fn build_unix_builders_validate_family_and_type() {
        let pool = FdInheritanceConfig::empty();
        let dir = tempfile::tempdir().unwrap();

        let listener =
            std::os::unix::net::UnixListener::bind(dir.path().join("stream.sock")).unwrap();
        let managed = UnixStreamSocketBuilder::build(&inherit(listener), "u", &pool).unwrap();
        assert!(managed.owned_socket_path().is_none());

        let dgram = std::os::unix::net::UnixDatagram::bind(dir.path().join("dgram.sock")).unwrap();
        let managed = UnixDatagramSocketBuilder::build(&inherit(dgram), "u", &pool).unwrap();
        assert!(managed.owned_socket_path().is_none());

        let (tcp, _) = tcp_listener_fd();
        assert_fd_inheritance_err(
            UnixStreamSocketBuilder::build(&inherit(tcp), "u", &pool),
            "expected AF_UNIX",
        );
        let udp = std::net::UdpSocket::bind(loopback()).unwrap();
        assert_fd_inheritance_err(
            UnixDatagramSocketBuilder::build(&inherit(udp), "u", &pool),
            "expected AF_UNIX",
        );
        let dgram = std::os::unix::net::UnixDatagram::unbound().unwrap();
        assert_fd_inheritance_err(
            UnixStreamSocketBuilder::build(&inherit(dgram), "u", &pool),
            "SOCK_STREAM",
        );
    }

    #[tokio::test]
    async fn build_rejected_fd_is_still_consumed() {
        // A descriptor that fails validation is closed, not returned to the
        // strategy, so a retry cannot pick it up again.
        let udp = std::net::UdpSocket::bind(loopback()).unwrap();
        let fd = InheritedFd::new(udp.into());
        let strategy = BindStrategy::Inherit(fd.clone());
        let pool = FdInheritanceConfig::empty();
        assert!(TcpSocketBuilder::build(&strategy, "tcp", &pool).is_err());
        assert!(fd.is_consumed());
        assert_fd_inheritance_err(
            TcpSocketBuilder::build(&strategy, "tcp", &pool),
            "already consumed",
        );
    }
}
