pub mod config;
pub mod datagram_protocol;
pub mod server;
pub mod tests;

pub use config::UdpConfig;
pub use datagram_protocol::UdpProtocol;
pub use server::UdpEchoServer;

// Type alias for the generic datagram client with UDP protocol
pub type UdpEchoClient = crate::datagram::DatagramEchoClient<UdpProtocol>;
