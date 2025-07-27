// Unix domain datagram socket protocol with file descriptor inheritance support
//
// Unix domain datagram sockets provide connectionless IPC (inter-process communication)
// on the same machine. Unlike stream sockets, datagram sockets preserve message boundaries
// and don't require connection establishment. They're useful for protocols that need
// discrete message delivery rather than stream-oriented communication.
//
// For zero-downtime reloads, Unix datagram sockets can be inherited from parent processes.
// Since datagrams are connectionless, the inherited socket can immediately receive messages
// without additional setup. Client-side sockets typically don't need inheritance since
// they're created per-connection or per-session.

use crate::datagram::protocol::DatagramProtocol;
use crate::network::socket_builder::BuildSocket;
use crate::network::fd_inheritance::BindTarget;
use crate::network::fd_inheritance::{BindStrategy, FdInheritanceConfig};
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::PathBuf;
use tokio::net::UnixDatagram;

/// Unix domain datagram socket builder
/// 
/// This builder handles creation of Unix domain datagram sockets with support for
/// file descriptor inheritance from parent processes. It validates that inherited
/// FDs are Unix domain datagram sockets.
pub struct UnixDatagramSocketBuilder;

impl BuildSocket<UnixDatagram> for UnixDatagramSocketBuilder {
    /// Unix domain datagram sockets use datagram sockets for message-oriented communication
    /// This preserves message boundaries unlike stream sockets
    const SOCKET_TYPE: libc::c_int = libc::SOCK_DGRAM;
    
    /// Unix domain sockets use the AF_UNIX address family
    /// They operate through kernel IPC mechanisms rather than network protocols
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_UNIX];
    
    /// Convert inherited file descriptor to Tokio UnixDatagram
    /// 
    /// This method assumes the FD has been validated as a Unix domain datagram socket.
    /// Unlike stream sockets, datagram sockets can immediately send/receive after
    /// inheritance without additional connection setup.
    /// 
    /// # Safety
    /// The file descriptor must be:
    /// - A valid socket file descriptor
    /// - A Unix domain datagram socket (AF_UNIX + SOCK_DGRAM)
    /// - Bound to a socket path (ready to receive messages)
    /// 
    /// # Arguments
    /// * `fd` - Raw file descriptor inherited from parent process
    fn from_fd(fd: RawFd) -> Result<UnixDatagram> {
        // Safety: FD has been validated by validate_inherited_fd()
        // Convert raw FD to std library UnixDatagram
        let std_socket = unsafe { std::os::unix::net::UnixDatagram::from_raw_fd(fd) };
        
        // Configure for async operation with Tokio
        // Tokio requires non-blocking sockets for proper async behavior
        std_socket.set_nonblocking(true)
            .map_err(|e| EchoError::Unix(e))?;
        
        // Convert std UnixDatagram to Tokio UnixDatagram
        // This registers the socket with Tokio's async runtime for efficient I/O
        UnixDatagram::from_std(std_socket)
            .map_err(|e| EchoError::Unix(e))
    }
    
    /// Create Unix datagram socket by binding to socket path
    /// 
    /// This method handles normal socket creation when inheritance is not
    /// available or not desired. It validates that the target is a Unix
    /// domain socket path (not a network address).
    /// 
    /// Like the stream protocol, we do NOT automatically remove existing socket files.
    /// This prevents race conditions and permission issues. The bind() syscall
    /// will fail cleanly if the path is already in use.
    /// 
    /// # Arguments
    /// * `target` - Where to bind the socket (must be Unix path)
    fn bind_to(target: &BindTarget) -> Result<UnixDatagram> {
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
                let std_socket = std::os::unix::net::UnixDatagram::bind(path)
                    .map_err(|e| EchoError::Unix(e))?;
                
                // Configure for async operation
                std_socket.set_nonblocking(true)
                    .map_err(|e| EchoError::Unix(e))?;
                
                // Convert to Tokio async UnixDatagram
                UnixDatagram::from_std(std_socket)
                    .map_err(|e| EchoError::Unix(e))
            }
            
            // Unix domain sockets cannot bind to network addresses
            BindTarget::Network(_addr) => {
                Err(EchoError::Config(
                    "Unix domain sockets cannot bind to network addresses. Use UdpSocketBuilder for network sockets.".into()
                ))
            }
        }
    }
}

/// Unix domain datagram protocol implementation
/// 
/// This protocol provides connectionless, message-oriented communication between
/// processes on the same machine using Unix domain sockets. Each send/receive
/// operation handles a complete message with preserved boundaries.
#[derive(Debug, Clone)]
pub struct UnixDatagramProtocol;

#[async_trait]
impl DatagramProtocol for UnixDatagramProtocol {
    type Error = crate::EchoError;
    type Socket = UnixDatagram;

    /// Bind Unix datagram socket with automatic FD inheritance detection
    /// 
    /// For Unix domain sockets, we adapt the DatagramConfig to work with our
    /// UnixDatagramConfig. This provides compatibility with the existing trait.
    async fn bind(config: &crate::datagram::DatagramConfig) -> std::result::Result<Self::Socket, Self::Error> {
        // Convert generic config to Unix-specific config
        // For now, use default Unix config since DatagramConfig doesn't have path info
        let unix_config = super::config::UnixDatagramConfig::default();
        
        // Detect FD inheritance from environment (systemd, etc.)
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        Self::bind_unix_with_inheritance(&unix_config, &fd_config).await
    }

    /// Bind Unix datagram socket with explicit FD inheritance configuration
    /// 
    /// This method provides compatibility with the DatagramProtocol trait while
    /// enabling Unix-specific FD inheritance functionality.
    async fn bind_with_inheritance(
        _config: &crate::datagram::DatagramConfig,
        fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Socket, Self::Error> {
        // Use default Unix config since generic DatagramConfig doesn't have path info
        let unix_config = super::config::UnixDatagramConfig::default();
        Self::bind_unix_with_inheritance(&unix_config, fd_config).await
    }

    /// Receive datagram message from Unix socket
    /// 
    /// Unix domain datagram sockets preserve message boundaries, so each
    /// receive operation gets exactly one complete message. For compatibility
    /// with the DatagramProtocol trait, we return a dummy SocketAddr.
    async fn recv_from(
        socket: &Self::Socket,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, std::net::SocketAddr), Self::Error> {
        // Receive message with sender information
        let (len, _sender_addr) = socket.recv_from(buffer).await
            .map_err(|e| EchoError::Unix(e))?;
        
        // Convert Unix socket address to dummy SocketAddr for trait compatibility
        // The actual peer address is not used in echo servers
        let dummy_addr = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 
            0
        );
        
        Ok((len, dummy_addr))
    }

    /// Send datagram message to Unix socket
    /// 
    /// For Unix domain datagram communication, we ignore the SocketAddr parameter
    /// and send to a default target path. Real Unix datagram applications should
    /// use the UnixDatagramExt trait for proper path-based addressing.
    async fn send_to(
        socket: &Self::Socket,
        data: &[u8],
        _addr: std::net::SocketAddr,
    ) -> std::result::Result<usize, Self::Error> {
        // For Unix domain datagrams, we need to send to a specific path
        // This is a limitation of the current trait design
        // In practice, UnixDatagramEchoClient will handle this differently
        let target_path = PathBuf::from("/tmp/echosrv_datagram.sock");
        
        socket.send_to(data, &target_path).await
            .map_err(|e| EchoError::Unix(e))
    }

    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error {
        EchoError::Unix(err)
    }
}

/// Extension trait for Unix domain datagram specific operations
/// 
/// This trait provides Unix-specific functionality for datagram sockets,
/// including client socket creation and abstract socket support.
pub trait UnixDatagramExt {
    /// Create unbound Unix datagram socket for client use
    /// 
    /// Creates a temporary socket for sending messages. The socket is bound to
    /// a temporary path so it can receive replies, but the path is managed
    /// automatically.
    async fn create_client_socket() -> Result<UnixDatagram>;
    
    /// Connect to Unix datagram socket using filesystem path
    /// 
    /// This creates a connected socket that can use send()/recv() instead of
    /// send_to()/recv_from() for slightly better performance.
    /// 
    /// # Arguments
    /// * `path` - Filesystem path to Unix domain socket
    async fn connect_unix(path: &PathBuf) -> Result<UnixDatagram>;
    
    /// Create abstract Unix datagram socket
    /// 
    /// Abstract sockets use names starting with null byte (\0) and don't
    /// create filesystem entries. They're useful for avoiding filesystem
    /// permission and cleanup issues.
    /// 
    /// # Arguments
    /// * `name` - Abstract socket name (without leading \0)
    async fn bind_abstract(name: &str) -> Result<UnixDatagram>;
}

impl UnixDatagramProtocol {
    /// Bind Unix datagram socket with explicit Unix configuration and FD inheritance
    /// 
    /// This method enables direct use of UnixDatagramConfig for better control over
    /// Unix domain socket specific features like socket paths and FD inheritance.
    pub async fn bind_unix_with_inheritance(
        config: &super::config::UnixDatagramConfig,
        fd_config: &FdInheritanceConfig,
    ) -> Result<UnixDatagram> {
        UnixDatagramSocketBuilder::build(
            &config.bind_strategy,
            &config.service_name,
            fd_config,
        )
    }
}

impl UnixDatagramExt for UnixDatagramProtocol {
    async fn create_client_socket() -> Result<UnixDatagram> {
        // Create temporary socket path for client
        let temp_dir = std::env::temp_dir();
        let client_path = temp_dir.join(format!(
            "echosrv_client_{}_{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        
        // Create client socket bound to temporary path
        let std_socket = std::os::unix::net::UnixDatagram::bind(&client_path)
            .map_err(|e| EchoError::Unix(e))?;
        
        std_socket.set_nonblocking(true)
            .map_err(|e| EchoError::Unix(e))?;
        
        UnixDatagram::from_std(std_socket)
            .map_err(|e| EchoError::Unix(e))
    }
    
    async fn connect_unix(path: &PathBuf) -> Result<UnixDatagram> {
        // Create temporary client socket first
        let client_socket = Self::create_client_socket().await?;
        
        // Connect to target socket (non-async method)
        client_socket.connect(path)
            .map_err(|e| EchoError::Unix(e))?;
        
        Ok(client_socket)
    }
    
    async fn bind_abstract(name: &str) -> Result<UnixDatagram> {
        // Abstract socket names start with null byte
        let abstract_name = format!("\0{}", name);
        
        let std_socket = std::os::unix::net::UnixDatagram::bind(abstract_name)
            .map_err(|e| EchoError::Unix(e))?;
        
        std_socket.set_nonblocking(true)
            .map_err(|e| EchoError::Unix(e))?;
        
        UnixDatagram::from_std(std_socket)
            .map_err(|e| EchoError::Unix(e))
    }
}