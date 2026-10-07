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
/// ```no_run
/// use echosrv::datagram::DatagramEchoClient;
/// use echosrv::common::EchoClient;
/// use echosrv::udp::UdpProtocol;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client: DatagramEchoClient<UdpProtocol> = DatagramEchoClient::connect(addr).await?;
///
///     let response = client.echo_string("Hello, World!").await?;
///     println!("Echo response: {}", response);
///     Ok(())
/// }
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
