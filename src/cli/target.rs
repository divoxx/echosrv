//! The positional target of both binaries: a port, `HOST:PORT` or a Unix
//! socket path, depending on the protocol; and the server's `--host`.

use super::protocol::Protocol;
use crate::defaults::{
    DEFAULT_HOST, DEFAULT_PORT, DEFAULT_UNIX_DGRAM_PATH, DEFAULT_UNIX_STREAM_PATH,
};
use std::ffi::OsStr;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// Where a server listens or a client connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// tcp/udp/http: a port, and the host if one was given. Without a host
    /// the server binds `--host` and the client connects to
    /// [`DEFAULT_HOST`].
    Net {
        /// Host name or address as written (`HOST` of `HOST:PORT`).
        host: Option<String>,
        port: u16,
    },
    /// unix-stream/unix-dgram: a socket path.
    Unix(PathBuf),
}

impl Target {
    /// The target of a server started with no arguments: [`DEFAULT_PORT`]
    /// or the protocol's default socket path.
    pub fn default_for(protocol: Protocol) -> Self {
        match protocol {
            Protocol::Tcp | Protocol::Udp | Protocol::Http => Self::Net {
                host: None,
                port: DEFAULT_PORT,
            },
            Protocol::UnixStream => Self::Unix(DEFAULT_UNIX_STREAM_PATH.into()),
            Protocol::UnixDatagram => Self::Unix(DEFAULT_UNIX_DGRAM_PATH.into()),
        }
    }

    /// Parses the server's target: a `PORT` for tcp/udp/http (the host is
    /// `--host`), or a socket path.
    ///
    /// # Errors
    ///
    /// `invalid port '<value>' (expected 0-65535)`, or an empty socket path.
    pub fn parse_listen(protocol: Protocol, value: &OsStr) -> Result<Self, String> {
        if protocol.is_unix() {
            return Self::unix(value);
        }
        let value = value.to_string_lossy();
        let port = value
            .parse()
            .map_err(|_| format!("invalid port '{value}' (expected 0-65535)"))?;
        Ok(Self::Net { host: None, port })
    }

    /// Parses the client's target: `HOST:PORT` or a bare `PORT` for
    /// tcp/udp/http, or a socket path.
    ///
    /// # Errors
    ///
    /// `<protocol> target must be HOST:PORT or PORT, got "<value>"`, or an
    /// empty socket path.
    pub fn parse_connect(protocol: Protocol, value: &OsStr) -> Result<Self, String> {
        if protocol.is_unix() {
            return Self::unix(value);
        }
        let value = value.to_string_lossy();
        if let Ok(port) = value.parse() {
            return Ok(Self::Net { host: None, port });
        }
        let host_port = match value.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && !value.starts_with('/') => {
                port.parse().ok().map(|port| (host, port))
            }
            _ => None,
        };
        match host_port {
            Some((host, port)) => Ok(Self::Net {
                host: Some(host.to_string()),
                port,
            }),
            None => Err(format!(
                "{protocol} target must be HOST:PORT or PORT, got {value:?}"
            )),
        }
    }

    fn unix(value: &OsStr) -> Result<Self, String> {
        if value.is_empty() {
            return Err("socket path must not be empty".into());
        }
        Ok(Self::Unix(PathBuf::from(value)))
    }
}

/// `HOST:PORT` ([`DEFAULT_HOST`] when no host was given) or the socket path.
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Net {
                host: Some(host),
                port,
            } => write!(f, "{host}:{port}"),
            Self::Net { host: None, port } => write!(f, "{}", SocketAddr::new(DEFAULT_HOST, *port)),
            Self::Unix(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Parses the server's `--host`. IPv6 addresses may be written in brackets
/// (`[::1]`).
///
/// # Errors
///
/// `invalid host address '<value>'` when it is not an IP address.
pub fn parse_host(value: &str) -> Result<IpAddr, String> {
    let trimmed = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    trimmed
        .parse()
        .map_err(|_| format!("invalid host address '{value}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(host: Option<&str>, port: u16) -> Target {
        Target::Net {
            host: host.map(str::to_string),
            port,
        }
    }

    fn listen(p: Protocol, v: &str) -> Result<Target, String> {
        Target::parse_listen(p, OsStr::new(v))
    }

    fn connect(p: Protocol, v: &str) -> Result<Target, String> {
        Target::parse_connect(p, OsStr::new(v))
    }

    #[test]
    fn defaults_match_the_server() {
        for p in [Protocol::Tcp, Protocol::Udp, Protocol::Http] {
            assert_eq!(Target::default_for(p).to_string(), "127.0.0.1:8080");
        }
        assert_eq!(
            Target::default_for(Protocol::UnixStream),
            Target::Unix(DEFAULT_UNIX_STREAM_PATH.into())
        );
        assert_eq!(
            Target::default_for(Protocol::UnixDatagram),
            Target::Unix(DEFAULT_UNIX_DGRAM_PATH.into())
        );
    }

    #[test]
    fn listen_targets_are_ports_or_paths() {
        assert_eq!(listen(Protocol::Udp, "9090"), Ok(net(None, 9090)));
        assert_eq!(listen(Protocol::Tcp, "0"), Ok(net(None, 0)));
        for bad in ["notaport", "70000", "-1", "", "host:80"] {
            let err = listen(Protocol::Tcp, bad).unwrap_err();
            assert_eq!(err, format!("invalid port '{bad}' (expected 0-65535)"));
        }
        assert_eq!(
            listen(Protocol::UnixDatagram, "/tmp/x.sock"),
            Ok(Target::Unix("/tmp/x.sock".into()))
        );
        assert!(
            listen(Protocol::UnixStream, "")
                .unwrap_err()
                .contains("empty")
        );
    }

    #[test]
    fn connect_targets() {
        assert_eq!(connect(Protocol::Udp, "9090"), Ok(net(None, 9090)));
        assert_eq!(
            connect(Protocol::Tcp, "host:1234"),
            Ok(net(Some("host"), 1234))
        );
        assert_eq!(
            connect(Protocol::Http, "[::1]:80"),
            Ok(net(Some("[::1]"), 80))
        );
        assert_eq!(
            connect(Protocol::Tcp, "host:1234").unwrap().to_string(),
            "host:1234"
        );
        assert_eq!(
            connect(Protocol::Tcp, "9091").unwrap().to_string(),
            "127.0.0.1:9091"
        );
        for bad in [
            "/tmp/echo.sock",
            "notaport",
            "70000",
            "host:",
            ":80",
            "host:x",
        ] {
            let err = connect(Protocol::Tcp, bad).unwrap_err();
            assert_eq!(
                err,
                format!("tcp target must be HOST:PORT or PORT, got {bad:?}")
            );
        }
        assert_eq!(
            connect(Protocol::UnixDatagram, "relative.sock"),
            Ok(Target::Unix("relative.sock".into()))
        );
        assert!(
            connect(Protocol::UnixStream, "")
                .unwrap_err()
                .contains("empty")
        );
    }

    #[test]
    fn host_addresses() {
        assert_eq!(parse_host("::1"), Ok("::1".parse().unwrap()));
        assert_eq!(parse_host("[::]"), Ok("::".parse().unwrap()));
        assert_eq!(parse_host("0.0.0.0"), Ok("0.0.0.0".parse().unwrap()));
        assert_eq!(
            parse_host("nope").unwrap_err(),
            "invalid host address 'nope'"
        );
    }
}
