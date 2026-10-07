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

#[tokio::test]
async fn test_unix_bind_target_rejected() {
    use crate::network::BindTarget;
    let config = TcpConfig {
        bind_strategy: Some(BindStrategy::Bind(BindTarget::Unix("/tmp/tcp.sock".into()))),
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    assert!(matches!(server.run().await, Err(EchoError::Config(_))));
}

#[tokio::test]
async fn test_bind_with_pool_uses_service_name() {
    use crate::network::FdInheritanceConfig;
    use crate::stream::StreamProtocol;
    use crate::tcp::TcpProtocol;

    let parent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let inherited_addr = parent.local_addr().unwrap();
    let pool = FdInheritanceConfig::from_fds([("web".to_string(), OwnedFd::from(parent))]);
    let config: crate::StreamConfig = TcpConfig::default().with_fd_inheritance("web").into();

    let listener = TcpProtocol::bind_with_inheritance(&config, &pool)
        .await
        .unwrap();
    assert_eq!(listener.local_addr().unwrap(), inherited_addr);

    // Pool drained: falls back to binding bind_addr (127.0.0.1:0).
    let fallback = TcpProtocol::bind_with_inheritance(&config, &pool)
        .await
        .unwrap();
    assert_ne!(fallback.local_addr().unwrap(), inherited_addr);
}

/// A library user with a single unnamed systemd descriptor ("unknown")
/// inherits it, like the CLI does, instead of binding.
#[tokio::test]
async fn test_bind_with_pool_uses_sole_unnamed_fd() {
    use crate::network::FdInheritanceConfig;
    use crate::network::fd_inheritance::SYSTEMD_UNNAMED_FD;
    use crate::stream::StreamProtocol;
    use crate::tcp::TcpProtocol;

    let parent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let inherited_addr = parent.local_addr().unwrap();
    let pool =
        FdInheritanceConfig::from_fds([(SYSTEMD_UNNAMED_FD.to_string(), OwnedFd::from(parent))]);
    let config: crate::StreamConfig = TcpConfig::default().with_fd_inheritance("tcp").into();

    let listener = TcpProtocol::bind_with_inheritance(&config, &pool)
        .await
        .unwrap();
    assert_eq!(listener.local_addr().unwrap(), inherited_addr);
    assert!(!pool.has_inherited_fds());
}

#[tokio::test]
async fn test_run_returns_ok_on_shutdown_while_running() {
    let server = std::sync::Arc::new(TcpEchoServer::new(TcpConfig::default().into()));
    let shutdown = server.shutdown_signal();
    let running = {
        let server = std::sync::Arc::clone(&server);
        tokio::spawn(async move { server.run().await })
    };
    // Let run() bind and start serving before signalling (the signal would
    // not be lost either way; see test_shutdown_before_run_is_not_lost).
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(!running.is_finished());
    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("run() did not stop")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn test_idle_connection_closed_after_read_timeout() {
    use tokio::io::AsyncReadExt;

    let config = TcpConfig {
        read_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    let mut raw = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(5), raw.read(&mut buf))
        .await
        .expect("idle connection was not closed")
        .unwrap();
    assert_eq!(n, 0);

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_connection_slot_released_after_disconnect() {
    let config = TcpConfig {
        max_connections: 1,
        ..Default::default()
    };
    let server = TcpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    for round in 0..3 {
        // Each client disconnects before the next one; the server may need a
        // moment to observe EOF and free the slot.
        let mut served = false;
        for _ in 0..50 {
            let mut client = TcpEchoClient::connect(addr).await.unwrap();
            let message = format!("round {round}");
            if matches!(client.echo_string(&message).await, Ok(ref s) if *s == message) {
                served = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(served, "slot not released before round {round}");
    }

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}
