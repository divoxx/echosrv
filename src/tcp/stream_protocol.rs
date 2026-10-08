//! [`StreamProtocol`] implementation for TCP.

use super::socket_builder::TcpSocketBuilder;
use crate::EchoError;
use crate::network::{BuildSocket, FdInheritanceConfig};
use crate::stream::{RejectReason, StreamConfig, StreamProtocol};
use async_trait::async_trait;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// TCP protocol implementation
///
/// Binding honors [`StreamConfig::bind_strategy`], so TCP listeners can be
/// inherited from systemd or another parent process. Connections rejected by
/// a rate limit are reset (see [`reject`](StreamProtocol::reject)).
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

    /// Raw TCP cannot signal a rejection in-band, so the connection is
    /// aborted instead: with `SO_LINGER` set to zero, closing it sends an RST.
    /// Clients see "connection reset", which they can tell apart from the
    /// orderly close (FIN) of an idle timeout or a server shutdown.
    async fn reject(
        stream: &mut TcpStream,
        _reason: RejectReason,
        _retry_after: Duration,
    ) -> std::result::Result<(), EchoError> {
        stream
            .set_linger(Some(Duration::ZERO))
            .map_err(EchoError::Tcp)
    }
}
