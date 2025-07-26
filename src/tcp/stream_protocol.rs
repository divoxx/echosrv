use crate::EchoError;
use crate::stream::{StreamProtocol, StreamConfig};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use async_trait::async_trait;

/// TCP protocol implementation
pub struct TcpProtocol;

#[async_trait]
impl StreamProtocol for TcpProtocol {
    type Error = EchoError;
    type Listener = TcpListener;
    type Stream = TcpStream;
    
    async fn bind(config: &StreamConfig) -> std::result::Result<TcpListener, EchoError> {
        TcpListener::bind(config.bind_addr)
            .await
            .map_err(|e| EchoError::Config(format!("Failed to bind TCP listener: {}", e)))
    }
    
    async fn accept(listener: &mut TcpListener) -> std::result::Result<(TcpStream, SocketAddr), EchoError> {
        listener.accept()
            .await
            .map_err(|e| EchoError::Tcp(e))
    }
    
    async fn connect(addr: SocketAddr) -> std::result::Result<TcpStream, EchoError> {
        TcpStream::connect(addr)
            .await
            .map_err(|e| EchoError::Config(format!("Failed to connect to {}: {}", addr, e)))
    }
    
    async fn read(stream: &mut TcpStream, buffer: &mut [u8]) -> std::result::Result<usize, EchoError> {
        stream.read(buffer).await.map_err(EchoError::Tcp)
    }
    
    async fn write(stream: &mut TcpStream, data: &[u8]) -> std::result::Result<(), EchoError> {
        stream.write_all(data).await.map_err(EchoError::Tcp)
    }
    
    async fn flush(stream: &mut TcpStream) -> std::result::Result<(), EchoError> {
        stream.flush().await.map_err(EchoError::Tcp)
    }
    
    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Tcp(err)
    }
} 