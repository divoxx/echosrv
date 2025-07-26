use crate::EchoError;
use crate::stream::{StreamConfig, StreamProtocol};
use async_trait::async_trait;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Unix domain stream protocol implementation
pub struct UnixStreamProtocol;

#[async_trait]
impl StreamProtocol for UnixStreamProtocol {
    type Error = EchoError;
    type Listener = UnixListener;
    type Stream = UnixStream;

    async fn bind(_config: &StreamConfig) -> std::result::Result<UnixListener, EchoError> {
        // For Unix domain sockets, we need to extract the socket path from the config
        // This is a bit of a hack since StreamConfig uses SocketAddr, but we need PathBuf
        // In practice, the UnixStreamEchoServer will use UnixStreamConfig directly
        let socket_path = PathBuf::from("/tmp/echosrv_stream.sock"); // Default fallback

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(&socket_path);

        UnixListener::bind(&socket_path).map_err(EchoError::Unix)
    }

    async fn accept(
        listener: &mut UnixListener,
    ) -> std::result::Result<(UnixStream, SocketAddr), EchoError> {
        let (stream, _addr) = listener.accept().await.map_err(EchoError::Unix)?;

        // Convert Unix socket address to a dummy SocketAddr for compatibility
        // The actual peer address is not used in echo servers
        let dummy_addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);

        Ok((stream, dummy_addr))
    }

    async fn connect(_addr: SocketAddr) -> std::result::Result<UnixStream, EchoError> {
        // For Unix domain sockets, we need to extract the socket path
        // This is a limitation of the current trait design
        // In practice, UnixStreamEchoClient will handle this differently
        let socket_path = PathBuf::from("/tmp/echosrv_stream.sock");

        UnixStream::connect(&socket_path)
            .await
            .map_err(EchoError::Unix)
    }

    async fn read(
        stream: &mut UnixStream,
        buffer: &mut [u8],
    ) -> std::result::Result<usize, EchoError> {
        stream.read(buffer).await.map_err(EchoError::Unix)
    }

    async fn write(stream: &mut UnixStream, data: &[u8]) -> std::result::Result<(), EchoError> {
        stream.write_all(data).await.map_err(EchoError::Unix)
    }

    async fn flush(stream: &mut UnixStream) -> std::result::Result<(), EchoError> {
        stream.flush().await.map_err(EchoError::Unix)
    }

    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Unix(err)
    }
}

// Extension trait to provide Unix-specific functionality
#[async_trait]
pub trait UnixStreamExt {
    /// Binds a Unix domain stream listener to the given socket path
    async fn bind_unix(socket_path: &Path) -> std::result::Result<UnixListener, EchoError>;

    /// Connects to a Unix domain stream server at the given socket path
    async fn connect_unix(socket_path: &Path) -> std::result::Result<UnixStream, EchoError>;
}

#[async_trait]
impl UnixStreamExt for UnixStreamProtocol {
    async fn bind_unix(socket_path: &Path) -> std::result::Result<UnixListener, EchoError> {
        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);

        UnixListener::bind(socket_path).map_err(EchoError::Unix)
    }

    async fn connect_unix(socket_path: &Path) -> std::result::Result<UnixStream, EchoError> {
        UnixStream::connect(socket_path)
            .await
            .map_err(EchoError::Unix)
    }
}

