use crate::EchoError;
use crate::common::protocols::{StreamProtocol, EchoConfig};
use super::config::TcpConfig;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// TCP protocol implementation
pub struct TcpProtocol;

impl StreamProtocol for TcpProtocol {
    type Config = TcpConfig;
    type Error = EchoError;
    type Listener = TcpListener;
    type Stream = TcpStream;
    
    fn bind(config: &TcpConfig) -> impl std::future::Future<Output = std::result::Result<TcpListener, EchoError>> + Send {
        async move {
            TcpListener::bind(config.bind_addr())
                .await
                .map_err(|e| EchoError::Config(format!("Failed to bind TCP listener: {}", e)))
        }
    }
    
    fn accept(listener: &mut TcpListener) -> impl std::future::Future<Output = std::result::Result<(TcpStream, SocketAddr), EchoError>> + Send {
        async move {
            listener.accept()
                .await
                .map_err(|e| EchoError::Tcp(e))
        }
    }
    
    fn read(stream: &mut TcpStream, buffer: &mut [u8]) -> impl std::future::Future<Output = std::result::Result<usize, EchoError>> + Send {
        async move {
            stream.read(buffer).await.map_err(EchoError::Tcp)
        }
    }
    
    fn write(stream: &mut TcpStream, data: &[u8]) -> impl std::future::Future<Output = std::result::Result<(), EchoError>> + Send {
        async move {
            stream.write_all(data).await.map_err(EchoError::Tcp)
        }
    }
    
    fn flush(stream: &mut TcpStream) -> impl std::future::Future<Output = std::result::Result<(), EchoError>> + Send {
        async move {
            stream.flush().await.map_err(EchoError::Tcp)
        }
    }
    
    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Tcp(err)
    }
} 