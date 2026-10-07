//! [`DatagramProtocol`] implementation for UDP.

use super::socket_builder::UdpSocketBuilder;
use crate::EchoError;
use crate::datagram::{DatagramConfig, DatagramProtocol};
use crate::network::{BuildSocket, FdInheritanceConfig};
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// UDP protocol implementation
///
/// Binding honors [`DatagramConfig::bind_strategy`], so UDP sockets can be
/// inherited from systemd or another parent process.
pub struct UdpProtocol;

#[async_trait]
impl DatagramProtocol for UdpProtocol {
    type Error = EchoError;
    type Socket = UdpSocket;
    type PeerAddr = SocketAddr;

    /// Binds using the process-wide systemd descriptor pool for name lookups.
    async fn bind(config: &DatagramConfig) -> std::result::Result<UdpSocket, EchoError> {
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        Self::bind_with_inheritance(config, &fd_config).await
    }

    async fn bind_with_inheritance(
        config: &DatagramConfig,
        fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<UdpSocket, EchoError> {
        UdpSocketBuilder::build(
            &config.effective_bind_strategy(),
            &config.service_name,
            fd_config,
        )
    }

    async fn recv_from(
        socket: &UdpSocket,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, SocketAddr), EchoError> {
        socket.recv_from(buffer).await.map_err(EchoError::Udp)
    }

    async fn send_to(
        socket: &UdpSocket,
        data: &[u8],
        addr: &SocketAddr,
    ) -> std::result::Result<usize, EchoError> {
        socket.send_to(data, addr).await.map_err(EchoError::Udp)
    }

    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Udp(err)
    }
}
