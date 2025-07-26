pub mod config;
pub mod server;
pub mod tests;
pub mod stream_protocol;

pub use config::TcpConfig;
pub use server::TcpEchoServer;
pub use stream_protocol::TcpProtocol;

// Type alias for the generic stream client with TCP protocol
pub type TcpEchoClient = crate::stream::StreamEchoClient<TcpProtocol>; 