//! UDP socket builder with file descriptor inheritance support.
//!
//! Inherited descriptors must be `SOCK_DGRAM` and `AF_INET`/`AF_INET6`.

use crate::network::fd_inheritance::BindTarget;
use crate::network::socket_builder::BuildSocket;
use crate::{EchoError, Result};
use std::os::fd::OwnedFd;
use tokio::net::UdpSocket;

/// UDP-specific socket builder
pub struct UdpSocketBuilder;

impl BuildSocket<UdpSocket> for UdpSocketBuilder {
    const SOCKET_TYPE: libc::c_int = libc::SOCK_DGRAM;
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_INET, libc::AF_INET6];

    /// Convert an inherited, validated descriptor into a Tokio `UdpSocket`.
    fn from_fd(fd: OwnedFd) -> Result<UdpSocket> {
        let std_socket = std::net::UdpSocket::from(fd);
        std_socket.set_nonblocking(true).map_err(EchoError::Udp)?;
        UdpSocket::from_std(std_socket).map_err(EchoError::Udp)
    }

    /// Bind a new socket; rejects Unix paths.
    fn bind_to(target: &BindTarget) -> Result<UdpSocket> {
        match target {
            BindTarget::Network(addr) => {
                let std_socket = std::net::UdpSocket::bind(addr).map_err(EchoError::Udp)?;
                std_socket.set_nonblocking(true).map_err(EchoError::Udp)?;
                UdpSocket::from_std(std_socket).map_err(EchoError::Udp)
            }
            BindTarget::Unix(path) => Err(EchoError::Config(format!(
                "UDP sockets cannot bind to Unix domain socket path {}",
                path.display()
            ))),
        }
    }
}
