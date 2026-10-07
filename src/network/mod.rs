//! Network addressing, socket construction and file descriptor inheritance

pub mod address;
pub mod fd_inheritance;
pub mod local_address;
pub mod socket_builder;

pub use address::Address;
pub use fd_inheritance::{BindStrategy, BindTarget, FdInheritanceConfig, InheritedFd};
pub use local_address::LocalAddress;
pub use socket_builder::{BuildSocket, SocketBuilder, SocketSource};
