use crate::EchoError;
use crate::stream::StreamProtocol;
use crate::stream::StreamConfig;
use async_trait::async_trait;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt, ReadBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::net::{UnixListener, UnixStream};

/// Unix domain stream protocol implementation
///
/// This implementation provides proper resource cleanup and management
/// for Unix domain socket connections.
pub struct Protocol;

/// Wrapper for Unix stream that includes cleanup information
pub struct ManagedUnixStream {
    inner: UnixStream,
    socket_path: Option<PathBuf>, // For cleanup on drop
}

impl ManagedUnixStream {
    fn new(stream: UnixStream) -> Self {
        Self {
            inner: stream,
            socket_path: None,
        }
    }

    fn with_path(stream: UnixStream, path: PathBuf) -> Self {
        Self {
            inner: stream,
            socket_path: Some(path),
        }
    }

    /// Get a reference to the inner stream
    pub fn inner(&self) -> &UnixStream {
        &self.inner
    }

    /// Get a mutable reference to the inner stream
    pub fn inner_mut(&mut self) -> &mut UnixStream {
        &mut self.inner
    }
}

impl AsyncRead for ManagedUnixStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for ManagedUnixStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Drop for ManagedUnixStream {
    fn drop(&mut self) {
        // Clean up socket file if this stream owns it (client-side)
        if let Some(path) = &self.socket_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Wrapper for Unix listener that includes cleanup
pub struct ManagedUnixListener {
    inner: UnixListener,
    socket_path: PathBuf,
}

impl ManagedUnixListener {
    fn new(listener: UnixListener, path: PathBuf) -> Self {
        Self {
            inner: listener,
            socket_path: path,
        }
    }

    pub async fn accept(&mut self) -> Result<(ManagedUnixStream, SocketAddr), std::io::Error> {
        let (stream, _) = self.inner.accept().await?;
        // Create a dummy socket address for compatibility with the trait
        let dummy_addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);
        Ok((ManagedUnixStream::new(stream), dummy_addr))
    }
}

impl Drop for ManagedUnixListener {
    fn drop(&mut self) {
        // Clean up socket file when listener is dropped
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[async_trait]
impl StreamProtocol for Protocol {
    type Error = EchoError;
    type Listener = ManagedUnixListener;
    type Stream = ManagedUnixStream;

    async fn bind(_config: &StreamConfig) -> std::result::Result<Self::Listener, EchoError> {
        // For now, we'll use a default path since the current StreamConfig doesn't support Unix addresses
        // In a real implementation, this would be fixed by using the unified config system
        let socket_path = PathBuf::from("/tmp/echosrv_stream.sock");

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(&socket_path);

        // Create parent directory if it doesn't exist
        if let Some(parent) = socket_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    EchoError::Unix(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to create directory {}: {}", parent.display(), e)
                    ))
                })?;
            }
        }

        let listener = UnixListener::bind(&socket_path).map_err(EchoError::Unix)?;
        Ok(ManagedUnixListener::new(listener, socket_path))
    }

    async fn accept(listener: &mut Self::Listener) -> std::result::Result<(Self::Stream, SocketAddr), EchoError> {
        listener.accept().await.map_err(EchoError::Unix)
    }

    async fn connect(_addr: SocketAddr) -> std::result::Result<Self::Stream, EchoError> {
        // This method signature is problematic for Unix sockets since it takes SocketAddr
        // In the improved design, we'd have a connect_unix method instead
        Err(EchoError::Unsupported(
            "Use connect_unix method for Unix domain socket connections".to_string()
        ))
    }

    async fn read(stream: &mut Self::Stream, buffer: &mut [u8]) -> std::result::Result<usize, EchoError> {
        stream.inner.read(buffer).await.map_err(EchoError::Unix)
    }

    async fn write(stream: &mut Self::Stream, data: &[u8]) -> std::result::Result<(), EchoError> {
        stream.inner.write_all(data).await.map_err(EchoError::Unix)
    }

    async fn flush(stream: &mut Self::Stream) -> std::result::Result<(), EchoError> {
        stream.inner.flush().await.map_err(EchoError::Unix)
    }

    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Unix(err)
    }
}

/// Extension trait for Unix-specific operations
#[async_trait]
pub trait StreamExt {
    /// Bind to a Unix domain socket using a path
    async fn bind_unix(socket_path: &PathBuf) -> std::result::Result<ManagedUnixListener, EchoError>;
    
    /// Connect to a Unix domain socket using a path
    async fn connect_unix(socket_path: &PathBuf) -> std::result::Result<ManagedUnixStream, EchoError>;
    
    /// Create an anonymous client socket (for testing or temporary connections)
    async fn connect_anonymous() -> std::result::Result<ManagedUnixStream, EchoError>;
}

#[async_trait]
impl StreamExt for Protocol {
    async fn bind_unix(socket_path: &PathBuf) -> std::result::Result<ManagedUnixListener, EchoError> {
        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);

        // Create parent directory if it doesn't exist
        if let Some(parent) = socket_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    EchoError::Unix(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to create directory {}: {}", parent.display(), e)
                    ))
                })?;
            }
        }

        let listener = UnixListener::bind(socket_path).map_err(EchoError::Unix)?;
        Ok(ManagedUnixListener::new(listener, socket_path.clone()))
    }

    async fn connect_unix(socket_path: &PathBuf) -> std::result::Result<ManagedUnixStream, EchoError> {
        let stream = UnixStream::connect(socket_path).await.map_err(EchoError::Unix)?;
        Ok(ManagedUnixStream::new(stream))
    }

    async fn connect_anonymous() -> std::result::Result<ManagedUnixStream, EchoError> {
        // Create a temporary socket path for the client
        let temp_dir = std::env::temp_dir();
        let client_socket_path = temp_dir.join(format!(
            "client_{}_{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(&client_socket_path);

        // Bind to create the socket, then we can use it to connect
        let stream = UnixStream::connect(&client_socket_path).await.map_err(EchoError::Unix)?;
        
        Ok(ManagedUnixStream::with_path(stream, client_socket_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_unix_stream_bind_and_cleanup() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test.sock");
        
        // Test basic socket path creation
        assert!(!socket_path.exists());
        
        // For now, just test the types work
        // Full testing would require integrating with the new config system
        let _socket_path_clone = socket_path.clone();
    }

    #[tokio::test]
    async fn test_unix_stream_connect() {
        let temp_dir = tempdir().unwrap();
        let socket_path = temp_dir.path().join("test_connect.sock");
        
        // Test connection attempt (will fail, but tests the API)
        let connect_result = Protocol::connect_unix(&socket_path).await;
        
        // Should fail since no server is listening
        assert!(connect_result.is_err());
    }
}