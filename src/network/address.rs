//! [`Address`]: a network socket address or a Unix socket path.

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
///
/// Returns [`EchoError::Config`](crate::EchoError::Config) for an invalid
/// socket address or an empty Unix path (`"unix:"`).
impl FromStr for Address {
    type Err = crate::EchoError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(stripped) = s.strip_prefix("unix:") {
            if stripped.is_empty() {
                return Err(crate::EchoError::Config(
                    "Invalid Unix socket address: empty path after 'unix:'".to_string(),
                ));
            }
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
    use crate::EchoError;
    use std::path::Path;

    #[test]
    fn parse_ipv4() {
        let addr: Address = "127.0.0.1:8080".parse().unwrap();
        assert_eq!(
            addr,
            Address::Network(SocketAddr::from(([127, 0, 0, 1], 8080)))
        );
        assert!(addr.is_network());
        assert!(!addr.is_unix());
        assert_eq!(addr.as_network().unwrap().port(), 8080);
        assert!(addr.as_unix().is_none());
    }

    #[test]
    fn parse_ipv6() {
        let addr = Address::try_from("[::1]:80").unwrap();
        let net = addr.as_network().unwrap();
        assert!(net.is_ipv6());
        assert!(net.ip().is_loopback());
        assert_eq!(net.port(), 80);

        let scoped: Address = "[fe80::1%1]:9000".parse().unwrap();
        assert!(scoped.as_network().unwrap().is_ipv6());
    }

    #[test]
    fn parse_unix_paths() {
        for (input, path) in [
            ("unix:/tmp/test.sock", "/tmp/test.sock"),
            ("unix:relative.sock", "relative.sock"),
            ("unix:/path with spaces/s.sock", "/path with spaces/s.sock"),
            // Only the first prefix is stripped.
            ("unix:unix:x", "unix:x"),
        ] {
            let addr: Address = input.parse().unwrap();
            assert!(addr.is_unix(), "{input}");
            assert!(!addr.is_network(), "{input}");
            assert_eq!(addr.as_unix().unwrap(), Path::new(path), "{input}");
            assert!(addr.as_network().is_none());
        }
    }

    #[test]
    fn parse_errors_are_config_errors() {
        for input in [
            "",
            "not-an-address",
            "127.0.0.1",
            "127.0.0.1:",
            "127.0.0.1:99999",
            "::1:80",
            "localhost:80",
            "/tmp/no-prefix.sock",
            "UNIX:/tmp/case.sock",
        ] {
            match input.parse::<Address>() {
                Err(EchoError::Config(msg)) => {
                    assert!(msg.contains("Invalid socket address"), "{input}: {msg}")
                }
                other => panic!("{input:?}: expected Config error, got {other:?}"),
            }
            assert!(Address::try_from(input).is_err(), "{input}");
        }
    }

    #[test]
    fn empty_unix_path_is_a_config_error() {
        match "unix:".parse::<Address>() {
            Err(EchoError::Config(msg)) => assert!(msg.contains("empty path"), "{msg}"),
            other => panic!("expected Config error, got {other:?}"),
        }
        assert!(matches!(
            Address::try_from("unix:"),
            Err(EchoError::Config(_))
        ));
        // A path that is only whitespace is unusual but not empty.
        assert_eq!(
            "unix: ".parse::<Address>().unwrap(),
            Address::Unix(PathBuf::from(" "))
        );
    }

    #[test]
    fn display() {
        let net_addr: Address = "127.0.0.1:8080".parse().unwrap();
        let v6: Address = "[::1]:80".parse().unwrap();
        let unix_addr: Address = "unix:/tmp/test.sock".parse().unwrap();

        assert_eq!(net_addr.to_string(), "127.0.0.1:8080");
        assert_eq!(v6.to_string(), "[::1]:80");
        assert_eq!(unix_addr.to_string(), "unix:/tmp/test.sock");
    }

    #[test]
    fn display_round_trips_through_from_str() {
        for input in [
            "0.0.0.0:0",
            "192.168.1.10:65535",
            "[::]:1",
            "[2001:db8::42]:8443",
            "unix:/run/echo.sock",
            "unix:rel/echo.sock",
        ] {
            let addr: Address = input.parse().unwrap();
            assert_eq!(addr.to_string(), input);
            assert_eq!(addr.to_string().parse::<Address>().unwrap(), addr);
        }
    }

    #[test]
    fn from_conversions() {
        let sock = SocketAddr::from(([10, 0, 0, 1], 7));
        assert_eq!(Address::from(sock), Address::Network(sock));

        let path = PathBuf::from("/tmp/a.sock");
        assert_eq!(Address::from(path.clone()), Address::Unix(path.clone()));
        assert_eq!(Address::from(path.as_path()), Address::Unix(path.clone()));

        let unix = Address::from(path.as_path());
        assert_eq!(unix.as_unix(), Some(&path));
        assert!(unix.as_network().is_none());
    }

    #[test]
    fn equality_distinguishes_kinds() {
        let net: Address = "127.0.0.1:1".parse().unwrap();
        let unix: Address = "unix:127.0.0.1:1".parse().unwrap();
        assert_ne!(net, unix);
        assert_eq!(net.clone(), net);
    }
}
