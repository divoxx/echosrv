//! The generic datagram echo client, [`DatagramEchoClient`].

use super::{DatagramClientConfig, DatagramConfig, DatagramProtocol};
use crate::common::EchoClient;
use crate::network::BindStrategy;
use crate::network::BindTarget;
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::time::timeout;

/// Generic datagram-based echo client for protocols addressed by [`SocketAddr`]
/// (e.g. UDP).
///
/// The client binds an ephemeral wildcard address of the same family as the
/// server (IPv4 or IPv6), sends each payload as one datagram and waits for the
/// reply.
///
/// # Examples
///
/// ```
/// use echosrv::datagram::DatagramEchoClient;
/// use echosrv::common::EchoClient;
/// use echosrv::udp::UdpProtocol;
/// # use echosrv::udp::{UdpConfig, UdpEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// # let server = UdpEchoServer::new(UdpConfig::default().into());
/// # let bound = server.bind().await?;
/// # let addr = *bound.local_addr().as_network().unwrap();
/// # tokio::spawn(bound.serve());
/// let mut client: DatagramEchoClient<UdpProtocol> = DatagramEchoClient::connect(addr).await?;
/// assert_eq!(client.echo_string("Hello, World!").await?, "Hello, World!");
/// # Ok(())
/// # }
/// ```
pub struct DatagramEchoClient<P: DatagramProtocol> {
    socket: P::Socket,
    server_addr: SocketAddr,
    config: DatagramClientConfig,
}

impl<P> DatagramEchoClient<P>
where
    P: DatagramProtocol<PeerAddr = SocketAddr>,
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Creates a client for the server at `server_addr` with default settings
    /// (64 KiB receive buffer, 5 s timeouts).
    pub async fn connect(server_addr: SocketAddr) -> Result<Self> {
        Self::connect_with_config(server_addr, DatagramClientConfig::default()).await
    }

    /// Creates a client for the server at `server_addr` with custom settings.
    pub async fn connect_with_config(
        server_addr: SocketAddr,
        config: DatagramClientConfig,
    ) -> Result<Self> {
        if config.buffer_size == 0 {
            return Err(EchoError::Config(
                "buffer_size must be greater than 0".into(),
            ));
        }
        let wildcard = if server_addr.is_ipv6() {
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
        } else {
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
        };
        let bind_config = DatagramConfig {
            bind_addr: wildcard,
            buffer_size: config.buffer_size,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            bind_strategy: Some(BindStrategy::Bind(BindTarget::Network(wildcard))),
            service_name: "datagram-client".to_string(),
        };

        let socket = P::bind(&bind_config).await.map_err(Into::into)?;

        Ok(Self {
            socket,
            server_addr,
            config,
        })
    }

    /// The client configuration.
    pub fn config(&self) -> &DatagramClientConfig {
        &self.config
    }
}

#[async_trait]
impl<P> EchoClient for DatagramEchoClient<P>
where
    P: DatagramProtocol<PeerAddr = SocketAddr>,
    P::Error: Into<EchoError> + std::fmt::Display,
{
    /// Sends `data` as one datagram and returns the echoed reply.
    ///
    /// Replies from addresses other than the server are ignored.
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        timeout(
            self.config.write_timeout,
            P::send_to(&self.socket, data, &self.server_addr),
        )
        .await
        .map_err(|_| EchoError::Timeout("Datagram send timeout".to_string()))?
        .map_err(Into::into)?;

        let mut buffer = vec![0; self.config.buffer_size];
        let receive = async {
            loop {
                let (n, from) = P::recv_from(&self.socket, &mut buffer).await?;
                if from == self.server_addr {
                    return Ok::<usize, P::Error>(n);
                }
            }
        };
        let n = timeout(self.config.read_timeout, receive)
            .await
            .map_err(|_| EchoError::Timeout("Datagram receive timeout".to_string()))?
            .map_err(Into::<EchoError>::into)?;

        buffer.truncate(n);
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::udp::UdpProtocol;
    use std::time::Duration;

    type UdpClient = DatagramEchoClient<UdpProtocol>;

    /// A UDP "server" socket the test drives by hand.
    async fn peer(addr: &str) -> Option<(tokio::net::UdpSocket, SocketAddr)> {
        let socket = tokio::net::UdpSocket::bind(addr).await.ok()?;
        let local = socket.local_addr().unwrap();
        Some((socket, local))
    }

    #[tokio::test]
    async fn zero_buffer_is_rejected() {
        let config = DatagramClientConfig {
            buffer_size: 0,
            ..Default::default()
        };
        let result = UdpClient::connect_with_config("127.0.0.1:9".parse().unwrap(), config).await;
        assert!(matches!(result, Err(EchoError::Config(_))));
    }

    #[tokio::test]
    async fn connect_uses_defaults_and_custom_config() {
        let client = UdpClient::connect("127.0.0.1:9".parse().unwrap())
            .await
            .unwrap();
        assert_eq!(client.config().buffer_size, 64 * 1024);
        assert_eq!(client.config().read_timeout, Duration::from_secs(5));

        let config = DatagramClientConfig {
            buffer_size: 10,
            read_timeout: Duration::from_millis(1),
            write_timeout: Duration::from_millis(2),
        };
        let client = UdpClient::connect_with_config("127.0.0.1:9".parse().unwrap(), config)
            .await
            .unwrap();
        assert_eq!(client.config().buffer_size, 10);
        assert_eq!(client.config().read_timeout, Duration::from_millis(1));
        assert_eq!(client.config().write_timeout, Duration::from_millis(2));
    }

    #[tokio::test]
    async fn binds_wildcard_of_server_family() {
        let v4 = UdpClient::connect("127.0.0.1:9".parse().unwrap())
            .await
            .unwrap();
        let local = v4.socket.local_addr().unwrap();
        assert!(local.is_ipv4());
        assert!(local.ip().is_unspecified());
        assert_ne!(local.port(), 0);

        if std::net::UdpSocket::bind("[::1]:0").is_err() {
            eprintln!("IPv6 unavailable; skipping IPv6 half");
            return;
        }
        let v6 = UdpClient::connect("[::1]:9".parse().unwrap())
            .await
            .unwrap();
        let local = v6.socket.local_addr().unwrap();
        assert!(local.is_ipv6());
        assert!(local.ip().is_unspecified());
    }

    #[tokio::test]
    async fn ipv6_round_trip() {
        let Some((server, server_addr)) = peer("[::1]:0").await else {
            eprintln!("IPv6 unavailable; skipping");
            return;
        };
        let mut client = UdpClient::connect(server_addr).await.unwrap();
        let reply = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            server.send_to(&buf[..n], from).await.unwrap();
        });
        assert_eq!(client.echo(b"v6").await.unwrap(), b"v6");
        reply.await.unwrap();
    }

    #[tokio::test]
    async fn times_out_when_no_reply() {
        let (_server, server_addr) = peer("127.0.0.1:0").await.unwrap();
        let config = DatagramClientConfig {
            read_timeout: Duration::from_secs(10),
            ..Default::default()
        };
        let mut client = UdpClient::connect_with_config(server_addr, config)
            .await
            .unwrap();

        // The paused clock auto-advances to the timeout once the runtime idles.
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        match client.echo(b"hello?").await {
            Err(EchoError::Timeout(msg)) => assert!(msg.contains("receive"), "{msg}"),
            other => panic!("expected Timeout, got {other:?}"),
        }
        assert!(started.elapsed() >= Duration::from_secs(10));
    }

    #[tokio::test]
    async fn ignores_datagrams_from_other_senders() {
        let (server, server_addr) = peer("127.0.0.1:0").await.unwrap();
        let (stranger, _) = peer("127.0.0.1:0").await.unwrap();
        let mut client = UdpClient::connect(server_addr).await.unwrap();
        let client_port = client.socket.local_addr().unwrap().port();
        let client_addr = SocketAddr::from(([127, 0, 0, 1], client_port));

        let script = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let (n, _) = server.recv_from(&mut buf).await.unwrap();
            // A spoofed reply arrives first; the client must skip it.
            stranger.send_to(b"spoof", client_addr).await.unwrap();
            server.send_to(&buf[..n], client_addr).await.unwrap();
        });
        assert_eq!(client.echo(b"real").await.unwrap(), b"real");
        script.await.unwrap();
    }

    #[tokio::test]
    async fn reply_larger_than_buffer_is_truncated() {
        let (server, server_addr) = peer("127.0.0.1:0").await.unwrap();
        let config = DatagramClientConfig {
            buffer_size: 4,
            ..Default::default()
        };
        let mut client = UdpClient::connect_with_config(server_addr, config)
            .await
            .unwrap();
        let reply = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            server.send_to(&buf[..n], from).await.unwrap();
        });
        assert_eq!(client.echo(b"0123456789").await.unwrap(), b"0123");
        reply.await.unwrap();
    }
}
