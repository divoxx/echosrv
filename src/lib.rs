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

    /// Timeout errors
    #[error("Timeout error: {0}")]
    Timeout(String),

    /// UTF-8 encoding errors
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    /// Unsupported operation errors
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
}

impl From<HttpProtocolError> for EchoError {
    fn from(err: HttpProtocolError) -> Self {
        match err {
            HttpProtocolError::Io(e) => EchoError::Tcp(e),
            HttpProtocolError::HttpParse(msg) => EchoError::Config(msg),
            HttpProtocolError::InvalidRequest(msg) => EchoError::Config(msg),
            HttpProtocolError::IncompleteRequest => {
                EchoError::Config("Incomplete HTTP request".to_string())
            }
        }
    }
}

/// Result type for the echosrv library
pub type Result<T> = std::result::Result<T, EchoError>;

pub mod common;
pub mod datagram;
pub mod http;
pub mod network;
pub mod performance;
pub mod security;
pub mod stream;
pub mod tcp;
pub mod udp;
pub mod unix;

// Re-export main types for convenience
pub use common::{EchoClient, EchoServerTrait};
pub use datagram::{DatagramConfig, DatagramEchoClient, DatagramEchoServer};
pub use http::{HttpConfig, HttpEchoClient, HttpEchoServer, HttpProtocol};
pub use network::Address;
pub use stream::{Client as StreamClient, StreamConfig, StreamEchoServer};
pub use tcp::{TcpConfig, TcpEchoClient, TcpEchoServer};
pub use udp::{UdpConfig, UdpEchoClient, UdpEchoServer};
pub use unix::{
    UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixStreamConfig,
    UnixStreamEchoClient, UnixStreamEchoServer,
};
