use super::socket_builder::TcpSocketBuilder;
use crate::EchoError;
use crate::network::{BuildSocket, FdInheritanceConfig};
use crate::stream::{StreamConfig, StreamProtocol};
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// TCP protocol implementation
///
/// Binding honors [`StreamConfig::bind_strategy`], so TCP listeners can be
/// inherited from systemd or another parent process.
pub struct TcpProtocol;

#[async_trait]
impl StreamProtocol for TcpProtocol {
    type Error = EchoError;
    type Listener = TcpListener;
    type Stream = TcpStream;

    /// Binds using the process-wide systemd descriptor pool for name lookups.
    async fn bind(config: &StreamConfig) -> std::result::Result<TcpListener, EchoError> {
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        Self::bind_with_inheritance(config, &fd_config).await
    }

    async fn bind_with_inheritance(
        config: &StreamConfig,
        fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<TcpListener, EchoError> {
        TcpSocketBuilder::build(
            &config.effective_bind_strategy(),
            &config.service_name,
            fd_config,
        )
    }

    async fn accept(
        listener: &mut TcpListener,
    ) -> std::result::Result<(TcpStream, SocketAddr), EchoError> {
        listener.accept().await.map_err(EchoError::Tcp)
    }

    async fn connect(addr: SocketAddr) -> std::result::Result<TcpStream, EchoError> {
        TcpStream::connect(addr).await.map_err(EchoError::Tcp)
    }

    async fn read(
        stream: &mut TcpStream,
        buffer: &mut [u8],
    ) -> std::result::Result<usize, EchoError> {
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
