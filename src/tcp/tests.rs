use crate::common::traits::EchoServerTrait;
use crate::network::{BindStrategy, InheritedFd};
use crate::{EchoClient, EchoError, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::os::fd::OwnedFd;
use std::time::Duration;

#[tokio::test]
async fn test_config_default() {
    let config = TcpConfig::default();
    assert_eq!(config.max_connections, 100);
    assert_eq!(config.buffer_size, 1024);
    assert_eq!(config.read_timeout, Duration::from_secs(30));
    assert_eq!(config.write_timeout, Duration::from_secs(30));
    assert!(config.bind_strategy.is_none());
}

#[tokio::test]
async fn test_shutdown_before_run_is_not_lost() {
    let server = TcpEchoServer::new(TcpConfig::default().into());
    server.shutdown_signal().send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server.run())
        .await
        .expect("run() ignored a shutdown sent before it started")
        .unwrap();
}

#[tokio::test]
async fn test_zero_buffer_rejected() {
    let config = TcpConfig {
        buffer_size: 0,
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    assert!(matches!(server.run().await, Err(EchoError::Config(_))));
}

#[tokio::test]
async fn test_inherited_listener_is_served_and_owned_once() {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = std_listener.local_addr().unwrap();
    let fd = InheritedFd::new(OwnedFd::from(std_listener));

    let config = TcpConfig {
        bind_strategy: Some(BindStrategy::Inherit(fd.clone())),
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr().as_network(), Some(&addr));
    assert!(fd.is_consumed());

    // A second bind with the same (consumed) descriptor must fail, not double-own it.
    assert!(matches!(
        server.bind().await,
        Err(EchoError::FdInheritance(_))
    ));

    let handle = tokio::spawn(bound.serve());
    let mut client = TcpEchoClient::connect(addr).await.unwrap();
    assert_eq!(client.echo_string("inherited").await.unwrap(), "inherited");

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_inherited_wrong_socket_type_rejected() {
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let config = TcpConfig {
        bind_strategy: Some(BindStrategy::Inherit(OwnedFd::from(udp).into())),
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    assert!(matches!(
        server.run().await,
        Err(EchoError::FdInheritance(_))
    ));
}

#[tokio::test]
async fn test_connection_limit_and_shutdown_cancels_connections() {
    let config = TcpConfig {
        max_connections: 1,
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    let mut first = TcpEchoClient::connect(addr).await.unwrap();
    assert_eq!(first.echo_string("one").await.unwrap(), "one");

    // Second connection exceeds the limit and is closed by the server.
    // (The client may see an error or an empty reply depending on timing.)
    let mut second = TcpEchoClient::connect(addr).await.unwrap();
    let rejected = second.echo_string("two").await;
    assert!(!matches!(rejected, Ok(ref s) if s == "two"));

    // Shutdown completes even though `first` is still connected and idle.
    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("shutdown did not cancel open connections")
        .unwrap()
        .unwrap();
}
