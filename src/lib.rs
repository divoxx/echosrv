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
