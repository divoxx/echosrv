use crate::EchoError;
use crate::datagram::{DatagramProtocol, DatagramConfig};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use async_trait::async_trait;

/// UDP protocol implementation
pub struct UdpProtocol;

#[async_trait]
impl DatagramProtocol for UdpProtocol {
    type Error = EchoError;
    type Socket = UdpSocket;
    
    async fn bind(config: &DatagramConfig) -> std::result::Result<UdpSocket, EchoError> {
        UdpSocket::bind(config.bind_addr)
            .await
            .map_err(|e| EchoError::Config(format!("Failed to bind UDP socket: {e}")))
    }
    
    async fn recv_from(socket: &UdpSocket, buffer: &mut [u8]) -> std::result::Result<(usize, SocketAddr), EchoError> {
        socket.recv_from(buffer).await.map_err(EchoError::Udp)
    }
    
    async fn send_to(socket: &UdpSocket, data: &[u8], addr: SocketAddr) -> std::result::Result<usize, EchoError> {
        socket.send_to(data, addr).await.map_err(EchoError::Udp)
    }
    
    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Udp(err)
    }
} 