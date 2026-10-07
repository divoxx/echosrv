//! TCP socket builder with file descriptor inheritance support.
//!
//! Inherited descriptors must be `SOCK_STREAM`, `AF_INET`/`AF_INET6` and
//! already listening.

use crate::network::fd_inheritance::BindTarget;
use crate::network::socket_builder::BuildSocket;
use crate::{EchoError, Result};
use std::os::fd::OwnedFd;
use tokio::net::TcpListener;

/// TCP-specific socket builder
pub struct TcpSocketBuilder;

impl BuildSocket<TcpListener> for TcpSocketBuilder {
    const SOCKET_TYPE: libc::c_int = libc::SOCK_STREAM;
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_INET, libc::AF_INET6];
    const REQUIRE_LISTENING: bool = true;

    /// Convert an inherited, validated descriptor into a Tokio `TcpListener`.
    fn from_fd(fd: OwnedFd) -> Result<TcpListener> {
        let std_listener = std::net::TcpListener::from(fd);
        std_listener.set_nonblocking(true).map_err(EchoError::Tcp)?;
        TcpListener::from_std(std_listener).map_err(EchoError::Tcp)
    }

    /// Bind a new listener; rejects Unix paths.
    fn bind_to(target: &BindTarget) -> Result<TcpListener> {
        match target {
            BindTarget::Network(addr) => {
                let std_listener = std::net::TcpListener::bind(addr).map_err(EchoError::Tcp)?;
                std_listener.set_nonblocking(true).map_err(EchoError::Tcp)?;
                TcpListener::from_std(std_listener).map_err(EchoError::Tcp)
            }
            BindTarget::Unix(path) => Err(EchoError::Config(format!(
                "TCP sockets cannot bind to Unix domain socket path {}",
                path.display()
            ))),
        }
    }
}
