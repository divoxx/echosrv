use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

/// Unified address type that supports both network and Unix domain sockets
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// Network address (TCP, UDP)
    Network(SocketAddr),
    /// Unix domain socket path
    Unix(PathBuf),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Network(addr) => write!(f, "{addr}"),
            Address::Unix(path) => write!(f, "unix:{}", path.display()),
        }
    }
}

impl From<SocketAddr> for Address {
    fn from(addr: SocketAddr) -> Self {
        Address::Network(addr)
    }
}

impl From<PathBuf> for Address {
    fn from(path: PathBuf) -> Self {
        Address::Unix(path)
    }
}

impl From<&std::path::Path> for Address {
    fn from(path: &std::path::Path) -> Self {
        Address::Unix(path.to_path_buf())
    }
}

/// Fallible conversion from a string.
///
/// Equivalent to [`str::parse`]: strings prefixed with `unix:` become
/// [`Address::Unix`], everything else must be a valid [`SocketAddr`].
///
/// # Examples
///
/// ```
/// use echosrv::Address;
///
/// let addr = Address::try_from("127.0.0.1:8080").unwrap();
/// assert!(addr.is_network());
/// assert!(Address::try_from("not an address").is_err());
/// ```
impl TryFrom<&str> for Address {
    type Error = crate::EchoError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Parses `unix:<path>` as [`Address::Unix`] and anything else as a
/// [`SocketAddr`] (e.g. `127.0.0.1:8080`, `[::1]:8080`).
impl FromStr for Address {
    type Err = crate::EchoError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(stripped) = s.strip_prefix("unix:") {
            Ok(Address::Unix(PathBuf::from(stripped)))
        } else {
            s.parse::<SocketAddr>()
                .map(Address::Network)
                .map_err(|e| crate::EchoError::Config(format!("Invalid socket address: {e}")))
        }
    }
}

impl Address {
    /// Returns true if this is a network address
    pub fn is_network(&self) -> bool {
        matches!(self, Address::Network(_))
    }

    /// Returns true if this is a Unix domain socket address
    pub fn is_unix(&self) -> bool {
        matches!(self, Address::Unix(_))
    }

    /// Get the network address if this is a network address
    pub fn as_network(&self) -> Option<&SocketAddr> {
        match self {
            Address::Network(addr) => Some(addr),
            _ => None,
        }
    }

    /// Get the Unix path if this is a Unix domain socket
    pub fn as_unix(&self) -> Option<&PathBuf> {
        match self {
            Address::Unix(path) => Some(path),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_address() {
        let addr: Address = "127.0.0.1:8080".parse().unwrap();
        assert!(addr.is_network());
        assert!(!addr.is_unix());
        assert!(addr.as_network().is_some());
        assert!(addr.as_unix().is_none());
    }

    #[test]
    fn test_unix_address() {
        let addr: Address = "unix:/tmp/test.sock".parse().unwrap();
        assert!(!addr.is_network());
        assert!(addr.is_unix());
        assert!(addr.as_network().is_none());
        assert!(addr.as_unix().is_some());
    }

    #[test]
    fn test_display() {
        let net_addr: Address = "127.0.0.1:8080".parse().unwrap();
        let unix_addr: Address = "unix:/tmp/test.sock".parse().unwrap();

        assert_eq!(net_addr.to_string(), "127.0.0.1:8080");
        assert_eq!(unix_addr.to_string(), "unix:/tmp/test.sock");
    }

    #[test]
    fn test_invalid_address_is_error_not_panic() {
        assert!("not-an-address".parse::<Address>().is_err());
        assert!(Address::try_from("127.0.0.1").is_err());
        let v6 = Address::try_from("[::1]:9000").unwrap();
        assert!(v6.as_network().unwrap().is_ipv6());
    }
}
