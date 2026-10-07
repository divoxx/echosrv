//! Unix domain stream socket protocol with file descriptor inheritance support.
//!
//! Unix domain sockets provide local IPC with filesystem-based access control.
//! Like network sockets they can be inherited from a parent process (e.g.
//! systemd `ListenStream=/run/echo.sock`). Inherited socket files belong to the
//! parent and are never removed by this crate; socket files the server binds
//! itself are removed when the listener is dropped.

use super::socket_file::{SocketFile, SocketKind, bind_with_stale_recovery};
use crate::network::fd_inheritance::BindTarget;
use crate::network::fd_inheritance::FdInheritanceConfig;
use crate::network::local_address::unix_address;
use crate::network::socket_builder::BuildSocket;
use crate::network::{Address, LocalAddress};
use crate::stream::StreamConfig;
use crate::stream::protocol::StreamProtocol;
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::os::fd::OwnedFd;
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};

/// A Unix stream listener that removes its socket file on drop if (and only
/// if) this process created it.
///
/// Inherited listeners never remove the socket file, since it belongs to the
/// parent process (e.g. systemd).
#[derive(Debug)]
pub struct ManagedUnixListener {
    // Field order matters: the socket is closed before the file is removed.
    listener: UnixListener,
    file: Option<SocketFile>,
}

impl ManagedUnixListener {
    /// The underlying Tokio listener.
    pub fn get_ref(&self) -> &UnixListener {
        &self.listener
    }

    /// The socket file that will be removed on drop, if this process bound it.
    pub fn owned_socket_path(&self) -> Option<&Path> {
        self.file.as_ref().map(SocketFile::path)
    }

    /// Accepts a new connection.
    pub async fn accept(&self) -> std::io::Result<(UnixStream, tokio::net::unix::SocketAddr)> {
        self.listener.accept().await
    }
}

impl LocalAddress for ManagedUnixListener {
    fn local_address(&self) -> std::io::Result<Address> {
        unix_address(&self.listener.local_addr()?)
    }
}

/// Unix domain stream socket builder
///
/// Inherited descriptors must be `AF_UNIX`, `SOCK_STREAM` and listening.
pub struct UnixStreamSocketBuilder;

impl BuildSocket<ManagedUnixListener> for UnixStreamSocketBuilder {
    const SOCKET_TYPE: libc::c_int = libc::SOCK_STREAM;
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_UNIX];
    const REQUIRE_LISTENING: bool = true;

    /// Convert an inherited, validated descriptor into a listener that does
    /// not own its socket file.
    fn from_fd(fd: OwnedFd) -> Result<ManagedUnixListener> {
        let std_listener = std::os::unix::net::UnixListener::from(fd);
        std_listener
            .set_nonblocking(true)
            .map_err(EchoError::Unix)?;
        Ok(ManagedUnixListener {
            listener: UnixListener::from_std(std_listener).map_err(EchoError::Unix)?,
            file: None,
        })
    }

    /// Bind a socket path, recovering a stale socket file left behind by a
    /// crashed server (only if nothing is listening on it).
    fn bind_to(target: &BindTarget) -> Result<ManagedUnixListener> {
        match target {
            BindTarget::Unix(path) => {
                let std_listener = bind_with_stale_recovery(path, SocketKind::Stream, |p| {
                    std::os::unix::net::UnixListener::bind(p)
                })
                .map_err(EchoError::Unix)?;
                let file = SocketFile::record(path).map_err(EchoError::Unix)?;
                std_listener
                    .set_nonblocking(true)
                    .map_err(EchoError::Unix)?;
                Ok(ManagedUnixListener {
                    listener: UnixListener::from_std(std_listener).map_err(EchoError::Unix)?,
                    file: Some(file),
                })
            }
            BindTarget::Network(addr) => Err(EchoError::Config(format!(
                "Unix domain sockets cannot bind to network address {addr}"
            ))),
        }
    }
}

/// Unix domain stream protocol implementation
///
/// Binding honors [`StreamConfig::bind_strategy`] (a Unix path or inherited
/// descriptor; see [`UnixStreamConfig`](super::UnixStreamConfig)).
#[derive(Debug, Clone)]
pub struct UnixStreamProtocol;

#[async_trait]
impl StreamProtocol for UnixStreamProtocol {
    type Error = crate::EchoError;
    type Listener = ManagedUnixListener;
    type Stream = UnixStream;

    /// Binds using the process-wide systemd descriptor pool for name lookups.
    async fn bind(config: &StreamConfig) -> std::result::Result<Self::Listener, Self::Error> {
        let fd_config = FdInheritanceConfig::from_systemd_env()?;
        Self::bind_with_inheritance(config, &fd_config).await
    }

    async fn bind_with_inheritance(
        config: &StreamConfig,
        fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Listener, Self::Error> {
        UnixStreamSocketBuilder::build(
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

    /// Accepts a connection. Unix peers have no IP address, so an unspecified
    /// `0.0.0.0:0` placeholder is returned for trait compatibility.
    async fn accept(
        listener: &mut Self::Listener,
    ) -> std::result::Result<(Self::Stream, std::net::SocketAddr), Self::Error> {
        let (stream, _addr) = listener.accept().await.map_err(EchoError::Unix)?;
        let placeholder =
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);
        Ok((stream, placeholder))
    }

    /// Always fails: Unix sockets are addressed by path. Use
    /// [`connect_address`](StreamProtocol::connect_address) with
    /// [`Address::Unix`] or [`UnixStreamExt::connect_unix`].
    async fn connect(
        _addr: std::net::SocketAddr,
    ) -> std::result::Result<Self::Stream, Self::Error> {
        Err(EchoError::Unsupported(
            "Unix domain sockets are addressed by path, not SocketAddr".to_string(),
        ))
    }

    async fn connect_address(addr: &Address) -> std::result::Result<Self::Stream, Self::Error> {
        match addr {
            Address::Unix(path) => UnixStream::connect(path).await.map_err(EchoError::Unix),
            Address::Network(addr) => Err(EchoError::Unsupported(format!(
                "Unix domain stream protocol cannot connect to network address {addr}"
            ))),
        }
    }

    async fn read(
        stream: &mut Self::Stream,
        buffer: &mut [u8],
    ) -> std::result::Result<usize, Self::Error> {
        use tokio::io::AsyncReadExt;
        stream.read(buffer).await.map_err(EchoError::Unix)
    }

    async fn write(stream: &mut Self::Stream, data: &[u8]) -> std::result::Result<(), Self::Error> {
        use tokio::io::AsyncWriteExt;
        stream.write_all(data).await.map_err(EchoError::Unix)
    }

    async fn flush(stream: &mut Self::Stream) -> std::result::Result<(), Self::Error> {
        use tokio::io::AsyncWriteExt;
        stream.flush().await.map_err(EchoError::Unix)
    }

    fn map_io_error(err: std::io::Error) -> Self::Error {
        EchoError::Unix(err)
    }
}

/// Extension trait for Unix domain socket specific operations
#[allow(async_fn_in_trait)]
pub trait UnixStreamExt {
    /// Connect to Unix domain socket using filesystem path
    async fn connect_unix(path: &Path) -> Result<UnixStream>;

    /// Connect to a Linux abstract-namespace socket (`name` without the leading NUL).
    #[cfg(target_os = "linux")]
    async fn connect_abstract(name: &str) -> Result<UnixStream>;
}

impl UnixStreamProtocol {
    /// Bind a Unix stream listener from a [`UnixStreamConfig`](super::UnixStreamConfig)
    /// with an explicit descriptor pool.
    pub async fn bind_unix_with_inheritance(
        config: &super::config::UnixStreamConfig,
        fd_config: &FdInheritanceConfig,
    ) -> Result<ManagedUnixListener> {
        UnixStreamSocketBuilder::build(&config.bind_strategy, &config.service_name, fd_config)
    }
}

impl UnixStreamExt for UnixStreamProtocol {
    async fn connect_unix(path: &Path) -> Result<UnixStream> {
        UnixStream::connect(path).await.map_err(EchoError::Unix)
    }

    #[cfg(target_os = "linux")]
    async fn connect_abstract(name: &str) -> Result<UnixStream> {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())
            .map_err(EchoError::Unix)?;
        let stream =
            std::os::unix::net::UnixStream::connect_addr(&addr).map_err(EchoError::Unix)?;
        stream.set_nonblocking(true).map_err(EchoError::Unix)?;
        UnixStream::from_std(stream).map_err(EchoError::Unix)
    }
}
