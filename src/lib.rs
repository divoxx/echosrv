#[cfg(not(unix))]
compile_error!("echosrv currently supports Unix-like platforms only (Linux, macOS, BSD)");

use crate::http::protocol::HttpProtocolError;
use thiserror::Error;

/// Error types for the echosrv library
#[derive(Error, Debug)]
pub enum EchoError {
    /// TCP-related errors (bind, connect, read, write)
    #[error("TCP error: {0}")]
    Tcp(#[from] std::io::Error),

    /// UDP-related errors (bind, send, receive)
    #[error("UDP error: {0}")]
    Udp(std::io::Error),

    /// Unix domain socket-related errors (bind, connect, read, write)
    #[error("Unix domain socket error: {0}")]
    Unix(std::io::Error),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// File descriptor inheritance errors
    #[error("FD inheritance error: {0}")]
    FdInheritance(String),

    /// Timeout errors
    #[error("Timeout error: {0}")]
    Timeout(String),

    /// UTF-8 encoding errors
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    /// Unsupported operation errors
    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    /// HTTP protocol errors (malformed or unsupported requests)
    #[error("HTTP error: {0}")]
    Http(String),
}

impl EchoError {
    /// Converts this error into a [`std::io::Error`].
    ///
    /// I/O-backed variants (`Tcp`, `Udp`, `Unix`) return the underlying error
    /// unchanged; every other variant is wrapped with [`std::io::Error::other`].
    /// This is useful when an `EchoError` must cross an API that only speaks
    /// `std::io::Error` (for example a protocol-specific error type).
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::EchoError;
    ///
    /// let err = EchoError::Config("bad".into()).into_io_error();
    /// assert_eq!(err.kind(), std::io::ErrorKind::Other);
    /// ```
    pub fn into_io_error(self) -> std::io::Error {
        match self {
            EchoError::Tcp(e) | EchoError::Udp(e) | EchoError::Unix(e) => e,
            other => std::io::Error::other(other.to_string()),
        }
    }
}

impl From<HttpProtocolError> for EchoError {
    fn from(err: HttpProtocolError) -> Self {
        match err {
            HttpProtocolError::Io(e) => EchoError::Tcp(e),
            other => EchoError::Http(other.to_string()),
        }
    }
}

/// Result type for the echosrv library
pub type Result<T> = std::result::Result<T, EchoError>;

pub mod common;
pub mod datagram;
pub mod http;
pub mod network;
pub mod stream;
pub mod tcp;
pub mod udp;
#[cfg(unix)]
pub mod unix;

// Re-export main types for convenience
pub use common::{EchoClient, EchoServerTrait};
pub use datagram::{DatagramConfig, DatagramEchoClient, DatagramEchoServer};
pub use http::{HttpConfig, HttpEchoClient, HttpEchoServer, HttpProtocol};
pub use network::Address;
pub use stream::{Client as StreamClient, StreamConfig, StreamEchoServer};
pub use tcp::{TcpConfig, TcpEchoClient, TcpEchoServer};
pub use udp::{UdpConfig, UdpEchoClient, UdpEchoServer};
#[cfg(unix)]
pub use unix::{
    UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixStreamConfig,
    UnixStreamEchoClient, UnixStreamEchoServer,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn io_err() -> io::Error {
        io::Error::new(io::ErrorKind::ConnectionRefused, "refused")
    }

    #[test]
    fn display_every_variant() {
        let utf8 = String::from_utf8(vec![0xff]).unwrap_err();
        let utf8_msg = utf8.to_string();
        let cases = [
            (EchoError::Tcp(io_err()), "TCP error: refused".to_string()),
            (EchoError::Udp(io_err()), "UDP error: refused".to_string()),
            (
                EchoError::Unix(io_err()),
                "Unix domain socket error: refused".to_string(),
            ),
            (
                EchoError::Config("bad".into()),
                "Configuration error: bad".to_string(),
            ),
            (
                EchoError::FdInheritance("fd".into()),
                "FD inheritance error: fd".to_string(),
            ),
            (
                EchoError::Timeout("slow".into()),
                "Timeout error: slow".to_string(),
            ),
            (EchoError::Utf8(utf8), format!("UTF-8 error: {utf8_msg}")),
            (
                EchoError::Unsupported("nope".into()),
                "Unsupported operation: nope".to_string(),
            ),
            (EchoError::Http("400".into()), "HTTP error: 400".to_string()),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn io_backed_variants_expose_source() {
        use std::error::Error as _;
        for err in [
            EchoError::Tcp(io_err()),
            EchoError::Udp(io_err()),
            EchoError::Unix(io_err()),
        ] {
            // Tcp uses #[from] (which implies #[source]); Udp/Unix do not
            // declare a source, so only check Display there.
            if matches!(err, EchoError::Tcp(_)) {
                assert!(err.source().is_some());
            }
            assert!(err.to_string().ends_with("refused"));
        }
        assert!(EchoError::Config("x".into()).source().is_none());
    }

    #[test]
    fn from_io_error_is_tcp() {
        let err: EchoError = io_err().into();
        match err {
            EchoError::Tcp(e) => assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused),
            other => panic!("expected Tcp, got {other:?}"),
        }
    }

    #[test]
    fn from_utf8_error() {
        let err: EchoError = String::from_utf8(vec![0xc3]).unwrap_err().into();
        assert!(matches!(err, EchoError::Utf8(_)));
    }

    #[test]
    fn from_http_protocol_error_maps_io_to_tcp_and_rest_to_http() {
        let err: EchoError = HttpProtocolError::Io(io_err()).into();
        assert!(
            matches!(err, EchoError::Tcp(ref e) if e.kind() == io::ErrorKind::ConnectionRefused)
        );

        let cases = [
            (
                HttpProtocolError::HttpParse("bad line".into()),
                "HTTP error: HTTP parsing error: bad line",
            ),
            (
                HttpProtocolError::InvalidRequest("too long".into()),
                "HTTP error: Invalid request: too long",
            ),
            (
                HttpProtocolError::IncompleteRequest,
                "HTTP error: Incomplete request",
            ),
        ];
        for (http, expected) in cases {
            let err: EchoError = http.into();
            assert!(matches!(err, EchoError::Http(_)), "{err:?}");
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn into_io_error_unwraps_io_variants() {
        for err in [
            EchoError::Tcp(io_err()),
            EchoError::Udp(io_err()),
            EchoError::Unix(io_err()),
        ] {
            let io = err.into_io_error();
            assert_eq!(io.kind(), io::ErrorKind::ConnectionRefused);
            assert_eq!(io.to_string(), "refused");
        }
    }

    #[test]
    fn into_io_error_wraps_other_variants() {
        let cases = [
            EchoError::Config("c".into()),
            EchoError::FdInheritance("f".into()),
            EchoError::Timeout("t".into()),
            EchoError::Utf8(String::from_utf8(vec![0xff]).unwrap_err()),
            EchoError::Unsupported("u".into()),
            EchoError::Http("h".into()),
        ];
        for err in cases {
            let message = err.to_string();
            let io = err.into_io_error();
            assert_eq!(io.kind(), io::ErrorKind::Other);
            assert_eq!(io.to_string(), message);
        }
    }
}
