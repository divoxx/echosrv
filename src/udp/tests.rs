use crate::common::traits::EchoServerTrait;
use crate::datagram::DatagramClientConfig;
use crate::{EchoClient, EchoError, UdpConfig, UdpEchoClient, UdpEchoServer};
use std::time::Duration;

#[tokio::test]
async fn test_config_default() {
    let config = UdpConfig::default();
    assert_eq!(config.buffer_size, 64 * 1024);
    assert_eq!(config.read_timeout, Duration::from_secs(30));
    assert_eq!(config.write_timeout, Duration::from_secs(30));
}

#[tokio::test]
async fn test_shutdown_before_run_is_not_lost() {
    let server = UdpEchoServer::new(UdpConfig::default().into());
    server.shutdown_signal().send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server.run())
        .await
        .expect("run() ignored a shutdown sent before it started")
        .unwrap();
}

#[tokio::test]
async fn test_zero_buffer_rejected() {
    let config = UdpConfig {
        buffer_size: 0,
        ..Default::default()
    };
    let server = UdpEchoServer::new(config.into());
    assert!(matches!(server.run().await, Err(EchoError::Config(_))));
}

async fn echo_large_datagram(bind: &str) {
    let config = UdpConfig {
        bind_addr: bind.parse().unwrap(),
        ..Default::default()
    };
    let server = UdpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    let mut client = UdpEchoClient::connect_with_config(
        addr,
        DatagramClientConfig {
            read_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let payload: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    assert_eq!(client.echo(&payload).await.unwrap(), payload);

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_large_datagram_ipv4() {
    echo_large_datagram("127.0.0.1:0").await;
}

#[tokio::test]
async fn test_large_datagram_ipv6() {
    if std::net::UdpSocket::bind("[::1]:0").is_err() {
        eprintln!("IPv6 loopback unavailable; skipping");
        return;
    }
    echo_large_datagram("[::1]:0").await;
}

type Running = (
    std::net::SocketAddr,
    tokio::sync::broadcast::Sender<()>,
    tokio::task::JoinHandle<crate::Result<()>>,
);

async fn start(config: UdpConfig) -> Running {
    let server = UdpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    let addr = *bound.local_addr().as_network().unwrap();
    (addr, shutdown, tokio::spawn(bound.serve()))
}

#[tokio::test]
async fn test_inherited_socket_is_served() {
    use crate::network::{BindStrategy, InheritedFd};
    use std::os::fd::OwnedFd;

    let parent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let parent_addr = parent.local_addr().unwrap();
    let fd = InheritedFd::new(OwnedFd::from(parent));
    let config = UdpConfig {
        bind_strategy: Some(BindStrategy::Inherit(fd.clone())),
        ..Default::default()
    };
    let (addr, shutdown, handle) = start(config).await;
    assert_eq!(addr, parent_addr);
    assert!(fd.is_consumed());

    let mut client = UdpEchoClient::connect(addr).await.unwrap();
    assert_eq!(client.echo_string("inherited").await.unwrap(), "inherited");

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_inherited_wrong_socket_type_rejected() {
    use crate::network::BindStrategy;
    use std::os::fd::OwnedFd;

    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = UdpConfig {
        bind_strategy: Some(BindStrategy::Inherit(OwnedFd::from(tcp).into())),
        ..Default::default()
    };
    let server = UdpEchoServer::new(config.into());
    assert!(matches!(
        server.run().await,
        Err(EchoError::FdInheritance(_))
    ));
}

#[tokio::test]
async fn test_unix_bind_target_rejected() {
    use crate::network::{BindStrategy, BindTarget};
    let config = UdpConfig {
        bind_strategy: Some(BindStrategy::Bind(BindTarget::Unix("/tmp/udp.sock".into()))),
        ..Default::default()
    };
    let server = UdpEchoServer::new(config.into());
    assert!(matches!(server.run().await, Err(EchoError::Config(_))));
}

#[tokio::test]
async fn test_idle_read_timeouts_do_not_stop_server() {
    let config = UdpConfig {
        read_timeout: Duration::from_millis(5),
        ..Default::default()
    };
    let (addr, shutdown, handle) = start(config).await;
    // Several idle receive timeouts elapse.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_finished());

    let mut client = UdpEchoClient::connect(addr).await.unwrap();
    assert_eq!(client.echo_string("still up").await.unwrap(), "still up");

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_concurrent_clients_get_their_own_replies() {
    let (addr, shutdown, handle) = start(UdpConfig::default()).await;
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            tokio::spawn(async move {
                let mut client = UdpEchoClient::connect(addr).await.unwrap();
                for j in 0..5 {
                    let message = format!("client {i} message {j}");
                    assert_eq!(client.echo_string(&message).await.unwrap(), message);
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_datagram_larger_than_server_buffer_is_truncated() {
    let config = UdpConfig {
        buffer_size: 8,
        ..Default::default()
    };
    let (addr, shutdown, handle) = start(config).await;
    let mut client = UdpEchoClient::connect(addr).await.unwrap();
    assert_eq!(client.echo(b"0123456789abcdef").await.unwrap(), b"01234567");
    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}
