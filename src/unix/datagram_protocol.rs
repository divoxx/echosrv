//! Unix domain datagram socket protocol with file descriptor inheritance support.
//!
//! Unix datagram sockets provide connectionless, message-oriented local IPC.
//! Replies can only be sent to peers bound to a filesystem path; the echo
//! clients in this crate bind a temporary path (removed on drop) for that
//! reason.

use super::socket_file::{SocketFile, SocketKind, bind_with_stale_recovery};
use crate::datagram::DatagramConfig;
use crate::datagram::protocol::DatagramProtocol;
use crate::network::fd_inheritance::BindTarget;
use crate::network::fd_inheritance::FdInheritanceConfig;
use crate::network::local_address::unix_address;
use crate::network::socket_builder::BuildSocket;
use crate::network::{Address, LocalAddress};
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::UnixDatagram;

/// A Unix datagram socket that removes its socket file on drop if (and only
/// if) this process created it.
#[derive(Debug)]
pub struct ManagedUnixDatagram {
    // Field order matters: the socket is closed before the file is removed.
    socket: UnixDatagram,
    file: Option<SocketFile>,
}

impl ManagedUnixDatagram {
    /// Wraps a socket that does not own any socket file.
    pub fn unmanaged(socket: UnixDatagram) -> Self {
        Self { socket, file: None }
    }

    /// The underlying Tokio socket.
    pub fn get_ref(&self) -> &UnixDatagram {
        &self.socket
    }

    /// The socket file that will be removed on drop, if this process bound it.
    pub fn owned_socket_path(&self) -> Option<&Path> {
        self.file.as_ref().map(SocketFile::path)
    }

    /// Binds `path` (recovering a stale socket file) and takes ownership of
    /// the created socket file.
    pub fn bind(path: &Path) -> std::io::Result<Self> {
        let std_socket = bind_with_stale_recovery(path, SocketKind::Datagram, |p| {
            std::os::unix::net::UnixDatagram::bind(p)
        })?;
        let file = SocketFile::record(path)?;
        std_socket.set_nonblocking(true)?;
        Ok(Self {
            socket: UnixDatagram::from_std(std_socket)?,
            file: Some(file),
        })
    }
}

impl LocalAddress for ManagedUnixDatagram {
    fn local_address(&self) -> std::io::Result<Address> {
        Ok(unix_address(self.socket.local_addr()?))
    }
}

/// Unix domain datagram socket builder
///
/// Inherited descriptors must be `AF_UNIX` and `SOCK_DGRAM`.
pub struct UnixDatagramSocketBuilder;

impl BuildSocket<ManagedUnixDatagram> for UnixDatagramSocketBuilder {
    const SOCKET_TYPE: libc::c_int = libc::SOCK_DGRAM;
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_UNIX];

    /// Convert an inherited, validated descriptor into a socket that does not
    /// own its socket file.
    fn from_fd(fd: OwnedFd) -> Result<ManagedUnixDatagram> {
        let std_socket = std::os::unix::net::UnixDatagram::from(fd);
        std_socket.set_nonblocking(true).map_err(EchoError::Unix)?;
        Ok(ManagedUnixDatagram::unmanaged(
            UnixDatagram::from_std(std_socket).map_err(EchoError::Unix)?,
        ))
    }

    /// Bind a socket path, recovering a stale socket file if nobody uses it.
    fn bind_to(target: &BindTarget) -> Result<ManagedUnixDatagram> {
        match target {
            BindTarget::Unix(path) => ManagedUnixDatagram::bind(path).map_err(EchoError::Unix),
            BindTarget::Network(addr) => Err(EchoError::Config(format!(
                "Unix domain sockets cannot bind to network address {addr}"
            ))),
        }
    }
}

/// Unix domain datagram protocol implementation
///
/// Binding honors [`DatagramConfig::bind_strategy`]. The peer address is the
/// sender's socket path (`None` for unnamed sockets, which cannot be replied to).
#[derive(Debug, Clone)]
pub struct UnixDatagramProtocol;

#[async_trait]
impl DatagramProtocol for UnixDatagramProtocol {
    type Error = crate::EchoError;
    type Socket = ManagedUnixDatagram;
    type PeerAddr = Option<PathBuf>;

    /// Binds using the process-wide systemd descriptor pool for name lookups.
    async fn bind(config: &DatagramConfig) -> std::result::Result<Self::Socket, Self::Error> {
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        Self::bind_with_inheritance(config, &fd_config).await
    }

    async fn bind_with_inheritance(
        config: &DatagramConfig,
        fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Socket, Self::Error> {
        UnixDatagramSocketBuilder::build(
            // A Unix socket has no use for the network `bind_addr`, so an
            // unset `InheritOrBind` fallback is not filled in from it.
            &config
                .bind_strategy
                .clone()
                .unwrap_or_else(|| config.effective_bind_strategy()),
            &config.service_name,
            fd_config,
        )
    }

    async fn recv_from(
        socket: &Self::Socket,
        buffer: &mut [u8],
    ) -> std::result::Result<(usize, Option<PathBuf>), Self::Error> {
        let (len, peer) = socket
            .socket
            .recv_from(buffer)
            .await
            .map_err(EchoError::Unix)?;
        Ok((len, peer.as_pathname().map(Path::to_path_buf)))
    }

    /// Sends to the peer's socket path; fails for unnamed peers.
    async fn send_to(
        socket: &Self::Socket,
        data: &[u8],
        addr: &Option<PathBuf>,
    ) -> std::result::Result<usize, Self::Error> {
        let path = addr.as_ref().ok_or_else(|| {
            EchoError::Unix(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "cannot reply to an unnamed Unix datagram socket; bind the client to a path",
            ))
        })?;
        socket
            .socket
            .send_to(data, path)
            .await
            .map_err(EchoError::Unix)
    }

    fn map_io_error(err: std::io::Error) -> Self::Error {
        EchoError::Unix(err)
    }
}

/// Extension trait for Unix domain datagram specific operations
#[allow(async_fn_in_trait)]
pub trait UnixDatagramExt {
    /// Create a client socket bound to a unique temporary path so it can
    /// receive replies. The path is removed when the socket is dropped.
    async fn create_client_socket() -> Result<ManagedUnixDatagram>;

    /// Create a client socket (see [`create_client_socket`](Self::create_client_socket))
    /// connected to `path`, for use with `send`/`recv`.
    ///
    /// Note: some platforms (e.g. macOS) reject `send_to` on a connected
    /// datagram socket with `EISCONN`; use `send` instead.
    async fn connect_unix(path: &Path) -> Result<ManagedUnixDatagram>;

    /// Bind a Linux abstract-namespace socket (`name` without the leading NUL).
    #[cfg(target_os = "linux")]
    async fn bind_abstract(name: &str) -> Result<UnixDatagram>;
}

impl UnixDatagramProtocol {
    /// Bind a Unix datagram socket from a
    /// [`UnixDatagramConfig`](super::UnixDatagramConfig) with an explicit descriptor pool.
    pub async fn bind_unix_with_inheritance(
        config: &super::config::UnixDatagramConfig,
        fd_config: &FdInheritanceConfig,
    ) -> Result<ManagedUnixDatagram> {
        UnixDatagramSocketBuilder::build(&config.bind_strategy, &config.service_name, fd_config)
    }
}

/// Returns a unique temporary socket path for a datagram client.
fn temp_client_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "echosrv-{}-{}-{}.sock",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        nanos
    ))
}

impl UnixDatagramExt for UnixDatagramProtocol {
    async fn create_client_socket() -> Result<ManagedUnixDatagram> {
        ManagedUnixDatagram::bind(&temp_client_path()).map_err(EchoError::Unix)
    }

    async fn connect_unix(path: &Path) -> Result<ManagedUnixDatagram> {
        let client = Self::create_client_socket().await?;
        client.socket.connect(path).map_err(EchoError::Unix)?;
        Ok(client)
    }

    #[cfg(target_os = "linux")]
    async fn bind_abstract(name: &str) -> Result<UnixDatagram> {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
            .map_err(EchoError::Unix)?;
        let socket = std::os::unix::net::UnixDatagram::bind_addr(&addr).map_err(EchoError::Unix)?;
        socket.set_nonblocking(true).map_err(EchoError::Unix)?;
        UnixDatagram::from_std(socket).map_err(EchoError::Unix)
    }
}
