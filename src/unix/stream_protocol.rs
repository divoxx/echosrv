// Unix domain stream socket protocol with file descriptor inheritance support
//
// Unix domain sockets provide local IPC (inter-process communication) on the same machine.
// They offer better performance than TCP for local communication and provide filesystem-based
// access control through socket file permissions.
//
// For zero-downtime reloads, Unix sockets can be inherited from parent processes just like
// network sockets. The parent process creates and binds the socket file, then passes the
// file descriptor to the child process. This enables seamless service restarts.
//
// Unlike network sockets, Unix socket inheritance has additional considerations:
// - Socket files have filesystem permissions that may affect inheritance
// - Abstract Unix sockets (starting with \0) don't use filesystem paths
// - Client connections don't create separate socket files

use crate::stream::protocol::StreamProtocol;
use crate::network::socket_builder::BuildSocket;
use crate::network::fd_inheritance::BindTarget;
use crate::network::fd_inheritance::{BindStrategy, FdInheritanceConfig};
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};

/// Unix domain stream socket builder
/// 
/// This builder handles creation of Unix domain stream listeners with support for
/// file descriptor inheritance from parent processes. It validates that inherited
/// FDs are Unix domain stream sockets.
pub struct UnixStreamSocketBuilder;

impl BuildSocket<UnixListener> for UnixStreamSocketBuilder {
    /// Unix domain sockets use stream sockets for reliable, ordered data delivery
    /// This is analogous to TCP but operates through filesystem/kernel IPC
    const SOCKET_TYPE: libc::c_int = libc::SOCK_STREAM;
    
    /// Unix domain sockets use the AF_UNIX address family
    /// Unlike network sockets, they only support one address family
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_UNIX];
    
    /// Convert inherited file descriptor to Tokio UnixListener
    /// 
    /// This method assumes the FD has been validated as a Unix domain stream socket.
    /// Unlike network sockets, Unix socket inheritance often comes from init systems
    /// that create the socket file with specific permissions and ownership.
    /// 
    /// # Safety
    /// The file descriptor must be:
    /// - A valid socket file descriptor
    /// - A Unix domain stream socket (AF_UNIX + SOCK_STREAM)
    /// - In listening state (listen() already called by parent)
    /// 
    /// # Arguments
    /// * `fd` - Raw file descriptor inherited from parent process
    fn from_fd(fd: RawFd) -> Result<UnixListener> {
        // Safety: FD has been validated by validate_inherited_fd()
        // Convert raw FD to std library UnixListener
        let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
        
        // Configure for async operation with Tokio
        // Tokio requires non-blocking sockets for proper async behavior
        std_listener.set_nonblocking(true)
            .map_err(|e| EchoError::Unix(e))?;
        
        // Convert std UnixListener to Tokio UnixListener
        // This registers the socket with Tokio's async runtime
        UnixListener::from_std(std_listener)
            .map_err(|e| EchoError::Unix(e))
    }
    
    /// Create Unix listener by binding to socket path
    /// 
    /// This method handles normal socket creation when inheritance is not
    /// available or not desired. It validates that the target is a Unix
    /// domain socket path (not a network address).
    /// 
    /// Key design decision: We do NOT automatically remove existing socket files.
    /// This prevents race conditions and permission issues. The bind() syscall
    /// will fail cleanly if the path is already in use, which is the correct
    /// behavior for robust service management.
    /// 
    /// # Arguments
    /// * `target` - Where to bind the socket (must be Unix path)
    fn bind_to(target: &BindTarget) -> Result<UnixListener> {
        match target {
            // Bind to Unix domain socket path
            BindTarget::Unix(path) => {
                // Create parent directory if it doesn't exist
                // This is safe because we only create the directory, not the socket file
                if let Some(parent) = path.parent() {
                    if !parent.exists() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| EchoError::Unix(e))?;
                    }
                }
                
                // Bind to socket path - let OS handle "already exists" errors
                // This is atomic and avoids race conditions from manual file removal
                let std_listener = std::os::unix::net::UnixListener::bind(path)
                    .map_err(|e| EchoError::Unix(e))?;
                
                // Configure for async operation
                std_listener.set_nonblocking(true)
                    .map_err(|e| EchoError::Unix(e))?;
                
                // Convert to Tokio async UnixListener
                UnixListener::from_std(std_listener)
                    .map_err(|e| EchoError::Unix(e))
            }
            
            // Unix domain sockets cannot bind to network addresses
            BindTarget::Network(_addr) => {
                Err(EchoError::Config(
                    "Unix domain sockets cannot bind to network addresses. Use TcpSocketBuilder for network sockets.".into()
                ))
            }
        }
    }
}

/// Unix domain stream protocol implementation
/// 
/// This protocol provides reliable, ordered communication between processes
/// on the same machine using Unix domain sockets. It supports both filesystem
/// socket paths and abstract socket names.
#[derive(Debug, Clone)]
pub struct UnixStreamProtocol;

#[async_trait]
impl StreamProtocol for UnixStreamProtocol {
    type Error = crate::EchoError;
    type Listener = UnixListener;
    type Stream = UnixStream;

    /// Bind Unix stream listener with automatic FD inheritance detection
    /// 
    /// For Unix domain sockets, we adapt the StreamConfig to work with our
    /// UnixStreamConfig. This provides compatibility with the existing trait.
    async fn bind(config: &crate::stream::StreamConfig) -> std::result::Result<Self::Listener, Self::Error> {
        // Convert generic config to Unix-specific config
        // For now, use default Unix config since StreamConfig doesn't have path info
        let unix_config = super::config::UnixStreamConfig::default();
        
        // Detect FD inheritance from environment (systemd, etc.)
        let fd_config = FdInheritanceConfig::from_systemd_env()
            .map_err(|e| EchoError::Unix(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        Self::bind_unix_with_inheritance(&unix_config, &fd_config).await
    }

    /// Bind Unix stream listener with explicit FD inheritance configuration
    /// 
    /// This method provides compatibility with the StreamProtocol trait while
    /// enabling Unix-specific FD inheritance functionality.
    async fn bind_with_inheritance(
        _config: &crate::stream::StreamConfig,
        fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Listener, Self::Error> {
        // Use default Unix config since generic StreamConfig doesn't have path info
        let unix_config = super::config::UnixStreamConfig::default();
        Self::bind_unix_with_inheritance(&unix_config, fd_config).await
    }

    /// Accept incoming connection from Unix stream listener
    /// 
    /// Unix domain socket connections don't have meaningful addresses like
    /// network sockets. We return a dummy SocketAddr for trait compatibility.
    async fn accept(
        listener: &mut Self::Listener,
    ) -> std::result::Result<(Self::Stream, std::net::SocketAddr), Self::Error> {
        let (stream, _addr) = listener.accept().await
            .map_err(|e| EchoError::Unix(e))?;
        
        // Create dummy SocketAddr for trait compatibility
        let dummy_addr = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 
            0
        );
        
        Ok((stream, dummy_addr))
    }

    /// Connect to a server at the given address (client-side)
    /// 
    /// For Unix domain sockets, SocketAddr doesn't make sense. This method
    /// will return an error. Use UnixStreamExt::connect_unix instead.
    async fn connect(_addr: std::net::SocketAddr) -> std::result::Result<Self::Stream, Self::Error> {
        Err(EchoError::Unsupported(
            "Use UnixStreamExt::connect_unix for Unix domain socket connections".to_string(),
        ))
    }

    /// Reads data from a stream
    async fn read(
        stream: &mut Self::Stream,
        buffer: &mut [u8],
    ) -> std::result::Result<usize, Self::Error> {
        use tokio::io::AsyncReadExt;
        stream.read(buffer).await.map_err(EchoError::Unix)
    }

    /// Writes data to a stream
    async fn write(stream: &mut Self::Stream, data: &[u8]) -> std::result::Result<(), Self::Error> {
        use tokio::io::AsyncWriteExt;
        stream.write_all(data).await.map_err(EchoError::Unix)
    }

    /// Flushes a stream
    async fn flush(stream: &mut Self::Stream) -> std::result::Result<(), Self::Error> {
        use tokio::io::AsyncWriteExt;
        stream.flush().await.map_err(EchoError::Unix)
    }

    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error {
        EchoError::Unix(err)
    }
}

/// Extension trait for Unix domain socket specific operations
/// 
/// This trait provides Unix-specific functionality that doesn't fit in the
/// generic StreamProtocol interface, such as connecting to socket paths
/// instead of network addresses.
pub trait UnixStreamExt {
    /// Connect to Unix domain socket using filesystem path
    /// 
    /// # Arguments
    /// * `path` - Filesystem path to Unix domain socket
    async fn connect_unix(path: &PathBuf) -> Result<UnixStream>;
    
    /// Connect to abstract Unix domain socket
    /// 
    /// Abstract sockets use names starting with null byte (\0) and don't
    /// create filesystem entries. They're useful for avoiding filesystem
    /// permission and cleanup issues.
    /// 
    /// # Arguments  
    /// * `name` - Abstract socket name (without leading \0)
    async fn connect_abstract(name: &str) -> Result<UnixStream>;
}

impl UnixStreamProtocol {
    /// Bind Unix stream listener with explicit Unix configuration and FD inheritance
    /// 
    /// This method enables direct use of UnixStreamConfig for better control over
    /// Unix domain socket specific features like socket paths and FD inheritance.
    pub async fn bind_unix_with_inheritance(
        config: &super::config::UnixStreamConfig,
        fd_config: &FdInheritanceConfig,
    ) -> Result<UnixListener> {
        UnixStreamSocketBuilder::build(
            &config.bind_strategy,
            &config.service_name,
            fd_config,
        )
    }
}

impl UnixStreamExt for UnixStreamProtocol {
    async fn connect_unix(path: &PathBuf) -> Result<UnixStream> {
        UnixStream::connect(path).await
            .map_err(|e| EchoError::Unix(e))
    }
    
    async fn connect_abstract(name: &str) -> Result<UnixStream> {
        // Abstract socket names start with null byte
        let abstract_name = format!("\0{}", name);
        UnixStream::connect(abstract_name).await
            .map_err(|e| EchoError::Unix(e))
    }
}