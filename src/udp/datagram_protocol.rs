use crate::EchoError;
use crate::datagram::{DatagramProtocol, DatagramConfig};
use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// UDP protocol implementation
pub struct UdpProtocol;

impl DatagramProtocol for UdpProtocol {
    type Error = EchoError;
    type Socket = UdpSocket;
    
    fn bind(config: &DatagramConfig) -> impl std::future::Future<Output = std::result::Result<UdpSocket, EchoError>> + Send {
        async move {
            UdpSocket::bind(config.bind_addr)
                .await
                .map_err(|e| EchoError::Config(format!("Failed to bind UDP socket: {}", e)))
        }
    }
    
    fn recv_from(socket: &UdpSocket, buffer: &mut [u8]) -> impl std::future::Future<Output = std::result::Result<(usize, SocketAddr), EchoError>> + Send {
        async move {
            socket.recv_from(buffer).await.map_err(EchoError::Udp)
        }
    }
    
    fn send_to(socket: &UdpSocket, data: &[u8], addr: SocketAddr) -> impl std::future::Future<Output = std::result::Result<usize, EchoError>> + Send {
        async move {
            socket.send_to(data, addr).await.map_err(EchoError::Udp)
        }
    }
    
    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Udp(err)
    }
} 