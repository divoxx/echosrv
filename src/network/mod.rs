//! Network addressing and configuration types

pub mod address;
pub mod config;
pub mod fd_inheritance;
pub mod socket_builder;

pub use address::Address;
pub use config::{Config, StreamConfig};
pub use fd_inheritance::{BindStrategy, BindTarget, FdInheritanceConfig};
pub use socket_builder::{BuildSocket, SocketBuilder, SocketSource};
