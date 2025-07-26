use crate::EchoError;
use crate::stream::{StreamProtocol, StreamConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Unix domain stream protocol implementation
pub struct UnixStreamProtocol;

impl StreamProtocol for UnixStreamProtocol {
    type Error = EchoError;
    type Listener = UnixListener;
    type Stream = UnixStream;
    
    fn bind(config: &StreamConfig) -> impl std::future::Future<Output = std::result::Result<UnixListener, EchoError>> + Send {
        async move {
            // For Unix domain sockets, we need to extract the socket path from the config
            // This is a bit of a hack since StreamConfig uses SocketAddr, but we need PathBuf
            // In practice, the UnixStreamEchoServer will use UnixStreamConfig directly
            let socket_path = PathBuf::from("/tmp/echosrv_stream.sock"); // Default fallback
            
            // Remove existing socket file if it exists
            let _ = std::fs::remove_file(&socket_path);
            
            UnixListener::bind(&socket_path)
                .map_err(|e| EchoError::Unix(e))
        }
    }
    
    fn accept(listener: &mut UnixListener) -> impl std::future::Future<Output = std::result::Result<(UnixStream, SocketAddr), EchoError>> + Send {
        async move {
            let (stream, _addr) = listener.accept()
                .await
                .map_err(|e| EchoError::Unix(e))?;
            
            // Convert Unix socket address to a dummy SocketAddr for compatibility
            // The actual peer address is not used in echo servers
            let dummy_addr = SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                0
            );
            
            Ok((stream, dummy_addr))
        }
    }
    
    async fn connect(_addr: SocketAddr) -> std::result::Result<UnixStream, EchoError> {
        // For Unix domain sockets, we need to extract the socket path
        // This is a limitation of the current trait design
        // In practice, UnixStreamEchoClient will handle this differently
        let socket_path = PathBuf::from("/tmp/echosrv_stream.sock");
        
        UnixStream::connect(&socket_path)
            .await
            .map_err(|e| EchoError::Unix(e))
    }
    
    fn read(stream: &mut UnixStream, buffer: &mut [u8]) -> impl std::future::Future<Output = std::result::Result<usize, EchoError>> + Send {
        async move {
            stream.read(buffer).await.map_err(EchoError::Unix)
        }
    }
    
    fn write(stream: &mut UnixStream, data: &[u8]) -> impl std::future::Future<Output = std::result::Result<(), EchoError>> + Send {
        async move {
            stream.write_all(data).await.map_err(EchoError::Unix)
        }
    }
    
    fn flush(stream: &mut UnixStream) -> impl std::future::Future<Output = std::result::Result<(), EchoError>> + Send {
        async move {
            stream.flush().await.map_err(EchoError::Unix)
        }
    }
    
    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Unix(err)
    }
}

// Extension trait to provide Unix-specific functionality
pub trait UnixStreamExt {
    /// Binds a Unix domain stream listener to the given socket path
    async fn bind_unix(socket_path: &PathBuf) -> std::result::Result<UnixListener, EchoError>;
    
    /// Connects to a Unix domain stream server at the given socket path
    async fn connect_unix(socket_path: &PathBuf) -> std::result::Result<UnixStream, EchoError>;
}

impl UnixStreamExt for UnixStreamProtocol {
    async fn bind_unix(socket_path: &PathBuf) -> std::result::Result<UnixListener, EchoError> {
        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);
        
        UnixListener::bind(socket_path)
            .map_err(EchoError::Unix)
    }
    
    async fn connect_unix(socket_path: &PathBuf) -> std::result::Result<UnixStream, EchoError> {
        UnixStream::connect(socket_path)
            .await
            .map_err(EchoError::Unix)
    }
} 