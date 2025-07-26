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

/// Result type for the echosrv library
pub type Result<T> = std::result::Result<T, EchoError>;

pub mod common;
pub mod datagram;
pub mod stream;
pub mod tcp;
pub mod udp;
pub mod unix;

// Re-export main types for convenience
pub use common::{EchoServerTrait, EchoClient};
pub use datagram::{DatagramConfig, DatagramEchoServer, DatagramEchoClient};
pub use stream::{StreamConfig, StreamEchoServer, StreamEchoClient};
pub use tcp::{TcpEchoServer, TcpEchoClient, TcpConfig};
pub use udp::{UdpEchoServer, UdpEchoClient, UdpConfig};
pub use unix::{UnixStreamEchoServer, UnixStreamEchoClient, UnixStreamConfig, UnixDatagramEchoServer, UnixDatagramEchoClient, UnixDatagramConfig}; 