//! Addressing, socket construction and file descriptor inheritance.
//!
//! * [`Address`]: a network address or a Unix socket path, accepted by the
//!   stream clients and returned by servers' `local_addr()`.
//! * [`fd_inheritance`]: using sockets created by a parent process
//!   (systemd socket activation, process managers) via [`BindStrategy`].
//! * [`socket_builder`]: the shared logic that turns a [`BindStrategy`] into
//!   a validated socket for each protocol.

pub mod address;
pub mod fd_inheritance;
pub mod local_address;
pub mod socket_builder;

pub use address::Address;
pub use fd_inheritance::{BindStrategy, BindTarget, FdInheritanceConfig, InheritedFd};
pub use local_address::LocalAddress;
pub use socket_builder::{BuildSocket, SocketBuilder, SocketSource};
