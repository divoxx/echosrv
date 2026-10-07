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
    ///   available, else take `service_name` from `fd_config`, else bind the
    ///   fallback target.
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
                    Ok(SocketSource::Inherit(owned))
                } else if let Some(owned) = fd_config.take(service_name) {
                    Ok(SocketSource::Inherit(owned))
                } else {
                    Ok(SocketSource::Bind(fallback_target.clone()))
                }
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
