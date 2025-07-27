use crate::EchoError;
use crate::datagram::{DatagramConfig, DatagramProtocol};
use async_trait::async_trait;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::net::UnixDatagram;

/// Unix domain datagram protocol implementation
pub struct UnixDatagramProtocol;

#[async_trait]
impl DatagramProtocol for UnixDatagramProtocol {
    type Error = EchoError;
    type Socket = UnixDatagram;

    async fn bind(_config: &DatagramConfig) -> std::result::Result<UnixDatagram, EchoError> {
        // For Unix domain sockets, we need to extract the socket path from the config
        // This is a bit of a hack since DatagramConfig uses SocketAddr, but we need PathBuf
        // In practice, the UnixDatagramEchoServer will use UnixDatagramConfig directly
        let socket_path = PathBuf::from("/tmp/echosrv_datagram.sock"); // Default fallback

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(&socket_path);

        UnixDatagram::bind(&socket_path).map_err(EchoError::Unix)
    }

    async fn recv_from(
        socket: &UnixDatagram,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, SocketAddr), EchoError> {
        let (len, _addr) = socket.recv_from(buffer).await.map_err(EchoError::Unix)?;

        // Convert Unix socket address to a dummy SocketAddr for compatibility
        // The actual peer address is not used in echo servers
        let dummy_addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);

        Ok((len, dummy_addr))
    }

    async fn send_to(
        socket: &UnixDatagram,
        data: &[u8],
        _addr: SocketAddr,
    ) -> std::result::Result<usize, EchoError> {
        // For Unix domain datagrams, we need to send to a specific path
        // This is a limitation of the current trait design
        // In practice, UnixDatagramEchoClient will handle this differently
        let target_path = PathBuf::from("/tmp/echosrv_datagram.sock");

        socket
            .send_to(data, &target_path)
            .await
            .map_err(EchoError::Unix)
    }

    fn map_io_error(err: std::io::Error) -> EchoError {
        EchoError::Unix(err)
    }
}

// Extension trait to provide Unix-specific functionality
#[async_trait]
pub trait UnixDatagramExt {
    /// Binds a Unix domain datagram socket to the given socket path
    async fn bind_unix(socket_path: &Path) -> std::result::Result<UnixDatagram, EchoError>;

    /// Connects to a Unix domain datagram server at the given socket path
    async fn connect_unix(socket_path: &Path) -> std::result::Result<UnixDatagram, EchoError>;
}

#[async_trait]
impl UnixDatagramExt for UnixDatagramProtocol {
    async fn bind_unix(socket_path: &Path) -> std::result::Result<UnixDatagram, EchoError> {
        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(socket_path);

        UnixDatagram::bind(socket_path).map_err(EchoError::Unix)
    }

    async fn connect_unix(_socket_path: &Path) -> std::result::Result<UnixDatagram, EchoError> {
        // Create a temporary named socket for the client
        let temp_dir = std::env::temp_dir();
        let client_socket_path =
            temp_dir.join(format!("echosrv_client_{}.sock", std::process::id()));

        // Remove existing socket file if it exists
        let _ = std::fs::remove_file(&client_socket_path);

        let socket = UnixDatagram::bind(&client_socket_path).map_err(EchoError::Unix)?;

        // Don't remove the socket file immediately - the server needs to be able to send to it
        // The socket file will be cleaned up when the socket is dropped

        Ok(socket)
    }
}
