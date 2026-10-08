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
/// Path sockets become [`Address::Unix`], Linux abstract-namespace sockets
/// [`Address::UnixAbstract`] and unbound sockets (e.g. from `socketpair(2)`)
/// [`Address::UnixUnnamed`].
#[cfg(unix)]
pub(crate) fn unix_address(addr: tokio::net::unix::SocketAddr) -> Address {
    let addr = std::os::unix::net::SocketAddr::from(addr);
    if let Some(path) = addr.as_pathname() {
        return Address::Unix(path.to_path_buf());
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        #[cfg(target_os = "android")]
        use std::os::android::net::SocketAddrExt;
        #[cfg(target_os = "linux")]
        use std::os::linux::net::SocketAddrExt;
        if let Some(name) = addr.as_abstract_name() {
            return Address::UnixAbstract(name.to_vec());
        }
    }
    Address::UnixUnnamed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unnamed_socket_address() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let addr = tokio::net::unix::SocketAddr::from(a.local_addr().unwrap());
        assert_eq!(unix_address(addr), Address::UnixUnnamed);
    }

    #[test]
    fn path_socket_address() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        let addr = tokio::net::unix::SocketAddr::from(socket.local_addr().unwrap());
        assert_eq!(unix_address(addr), Address::Unix(path));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn abstract_socket_address() {
        use std::os::linux::net::SocketAddrExt;
        let name = format!("echosrv-test-local-address-{}", std::process::id());
        let bind_addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
        let socket = std::os::unix::net::UnixDatagram::bind_addr(&bind_addr).unwrap();
        let addr = tokio::net::unix::SocketAddr::from(socket.local_addr().unwrap());
        assert_eq!(unix_address(addr), Address::UnixAbstract(name.into_bytes()));
    }
}
