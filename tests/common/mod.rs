use echosrv::Result;
use echosrv::common::EchoServerTrait;
use std::net::SocketAddr;
use tokio::task::JoinHandle;

/// Starts a TCP echo server on `127.0.0.1:0` with the given connection limit.
///
/// The server is bound before this function returns, so the returned address
/// is the real one and is already accepting connections (no sleeps, no
/// bind/drop/rebind race).
#[allow(dead_code)]
pub async fn create_controlled_test_server_with_limit(
    max_connections: usize,
) -> Result<(JoinHandle<Result<()>>, SocketAddr)> {
    let (handle, addr, _shutdown) = start_tcp_server(max_connections).await?;
    Ok((handle, addr))
}

/// Like [`create_controlled_test_server_with_limit`] but also returns the
/// shutdown sender for graceful termination.
#[allow(dead_code)]
pub async fn start_tcp_server(
    max_connections: usize,
) -> Result<(
    JoinHandle<Result<()>>,
    SocketAddr,
    tokio::sync::broadcast::Sender<()>,
)> {
    use echosrv::{TcpConfig, TcpEchoServer};

    let config = TcpConfig {
        max_connections,
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await?;
    let addr = *bound
        .local_addr()
        .as_network()
        .expect("TCP server has a network address");
    let handle = tokio::spawn(bound.serve());
    Ok((handle, addr, shutdown))
}
