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
