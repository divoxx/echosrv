pub mod config;
pub mod server;
pub mod tests;
pub mod datagram_protocol;

pub use config::UdpConfig;
pub use server::UdpEchoServer;
pub use datagram_protocol::UdpProtocol;

// Type alias for the generic datagram client with UDP protocol
pub type UdpEchoClient = crate::datagram::DatagramEchoClient<UdpProtocol>; 