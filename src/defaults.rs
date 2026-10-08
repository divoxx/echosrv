//! Default endpoints shared by the `echosrv` binary and the library.
//!
//! The `echosrv` command line uses these when no protocol, port, host or
//! socket path is given, and the Unix configs
//! ([`UnixStreamConfig`](crate::UnixStreamConfig),
//! [`UnixDatagramConfig`](crate::UnixDatagramConfig)) bind the default socket
//! paths. A client that uses the same constants reaches a server started
//! with no arguments.
//!
//! Note that the network configs ([`TcpConfig`](crate::TcpConfig) and
//! friends) default to `127.0.0.1:0`, an ephemeral port, rather than to
//! [`DEFAULT_PORT`].
//!
//! # Examples
//!
//! ```
//! use echosrv::defaults::{DEFAULT_HOST, DEFAULT_PORT};
//! use std::net::SocketAddr;
//!
//! let addr = SocketAddr::new(DEFAULT_HOST, DEFAULT_PORT);
//! assert_eq!(addr.to_string(), "127.0.0.1:8080");
//! ```

use std::net::{IpAddr, Ipv4Addr};

/// Protocol served when none is given: `tcp`.
pub const DEFAULT_PROTOCOL: &str = "tcp";

/// IP address to bind (server) or connect to (client) for tcp/udp/http:
/// `127.0.0.1`.
pub const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Port for tcp/udp/http: `8080`.
pub const DEFAULT_PORT: u16 = 8080;

/// Socket path of the Unix domain stream server: `/tmp/echosrv_stream.sock`.
pub const DEFAULT_UNIX_STREAM_PATH: &str = "/tmp/echosrv_stream.sock";

/// Socket path of the Unix domain datagram server:
/// `/tmp/echosrv_datagram.sock`.
pub const DEFAULT_UNIX_DGRAM_PATH: &str = "/tmp/echosrv_datagram.sock";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{BindStrategy, BindTarget};

    #[test]
    fn values() {
        assert_eq!(DEFAULT_PROTOCOL, "tcp");
        assert_eq!(DEFAULT_HOST.to_string(), "127.0.0.1");
        assert_eq!(DEFAULT_PORT, 8080);
    }

    #[test]
    fn unix_configs_bind_the_default_paths() {
        for (strategy, expected) in [
            (
                crate::UnixStreamConfig::default().bind_strategy,
                DEFAULT_UNIX_STREAM_PATH,
            ),
            (
                crate::UnixDatagramConfig::default().bind_strategy,
                DEFAULT_UNIX_DGRAM_PATH,
            ),
        ] {
            match strategy {
                BindStrategy::Bind(BindTarget::Unix(path)) => {
                    assert_eq!(path.as_os_str(), expected)
                }
                other => panic!("unexpected default strategy {other:?}"),
            }
        }
    }
}
