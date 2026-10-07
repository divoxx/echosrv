//! Querying the local address a server socket is bound to.

use super::Address;
use std::io;

/// Socket types that can report the address they are bound to.
///
/// Used by servers to expose the actual bound address (important when binding
/// port `0` or inheriting a descriptor).
pub trait LocalAddress {
    /// Returns the local address of the socket.
    fn local_address(&self) -> io::Result<Address>;
}

impl LocalAddress for tokio::net::TcpListener {
    fn local_address(&self) -> io::Result<Address> {
        self.local_addr().map(Address::Network)
    }
}

impl LocalAddress for tokio::net::UdpSocket {
    fn local_address(&self) -> io::Result<Address> {
        self.local_addr().map(Address::Network)
    }
}

/// Converts a Tokio Unix socket address into an [`Address`].
///
/// Unnamed (and abstract) sockets have no filesystem path and are reported as
/// an error of kind [`io::ErrorKind::AddrNotAvailable`].
#[cfg(unix)]
pub(crate) fn unix_address(addr: &tokio::net::unix::SocketAddr) -> io::Result<Address> {
    addr.as_pathname()
        .map(|path| Address::Unix(path.to_path_buf()))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "Unix socket has no filesystem path",
            )
        })
}
