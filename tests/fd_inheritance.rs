//! End-to-end socket inheritance: a socket created by "the parent" (this test)
//! is handed to a server via `BindStrategy::Inherit` and served.

mod common;

use common::{
    socket_dir, start_http, start_tcp, start_udp, start_unix_datagram, start_unix_stream,
};
use echosrv::http::{HttpConfig, HttpEchoClient};
use echosrv::network::{BindStrategy, BindTarget, InheritedFd};
use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
use echosrv::{
    EchoClient, EchoError, TcpConfig, TcpEchoClient, TcpEchoServer, UdpConfig, UdpEchoClient,
    UdpEchoServer, UnixDatagramEchoClient, UnixStreamEchoClient, UnixStreamEchoServer,
};
use std::net::SocketAddr;
use std::os::fd::OwnedFd;

/// An address we never expect the server to bind when it inherits.
fn unused_bind_addr() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

fn inherit(fd: impl Into<OwnedFd>) -> Option<BindStrategy> {
    Some(BindStrategy::Inherit(InheritedFd::from(fd.into())))
}

#[tokio::test]
async fn tcp_serves_inherited_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = start_tcp(TcpConfig {
        bind_addr: unused_bind_addr(),
        bind_strategy: inherit(listener),
        ..Default::default()
    })
    .await;
    assert_eq!(server.addr, addr, "server reports the inherited address");

    let mut client = TcpEchoClient::connect(addr).await.unwrap();
    assert_eq!(
        client.echo_string("inherited tcp").await.unwrap(),
        "inherited tcp"
    );
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn udp_serves_inherited_socket() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();

    let server = start_udp(UdpConfig {
        bind_addr: unused_bind_addr(),
        bind_strategy: inherit(socket),
        ..Default::default()
    })
    .await;
    assert_eq!(server.addr, addr);

    let mut client = UdpEchoClient::connect(addr).await.unwrap();
    assert_eq!(
        client.echo_string("inherited udp").await.unwrap(),
        "inherited udp"
    );
    server.stop().await;
}

#[tokio::test]
async fn http_serves_inherited_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = start_http(HttpConfig {
        bind_addr: unused_bind_addr(),
        bind_strategy: inherit(listener),
        ..Default::default()
    })
    .await;
    assert_eq!(server.addr, addr);

    let mut client = HttpEchoClient::connect(addr).await.unwrap();
    assert_eq!(
        client.echo(b"inherited http").await.unwrap(),
        b"inherited http"
    );
    server.stop().await;
}

#[tokio::test]
async fn unix_stream_serves_inherited_listener_and_keeps_its_file() {
    let dir = socket_dir();
    let path = dir.path().join("inherited.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

    let server = start_unix_stream(UnixStreamConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::from(OwnedFd::from(listener))),
        ..Default::default()
    })
    .await;
    assert_eq!(server.addr, path);

    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(
        client.echo_string("inherited unix").await.unwrap(),
        "inherited unix"
    );
    drop(client);
    server.stop().await;
    assert!(
        path.exists(),
        "the socket file of an inherited listener belongs to the parent"
    );
}

#[tokio::test]
async fn unix_datagram_serves_inherited_socket_and_keeps_its_file() {
    let dir = socket_dir();
    let path = dir.path().join("inherited.sock");
    let socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();

    let server = start_unix_datagram(UnixDatagramConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::from(OwnedFd::from(socket))),
        ..Default::default()
    })
    .await;
    assert_eq!(server.addr, path);

    let mut client = UnixDatagramEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(
        client.echo_string("inherited dgram").await.unwrap(),
        "inherited dgram"
    );
    drop(client);
    server.stop().await;
    assert!(path.exists());
}

#[tokio::test]
async fn inherited_fd_is_consumed_once() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let fd = InheritedFd::from(OwnedFd::from(listener));

    let first = start_tcp(TcpConfig {
        bind_strategy: Some(BindStrategy::Inherit(fd.clone())),
        ..Default::default()
    })
    .await;
    assert!(fd.is_consumed());

    // A second server given the same handle must not double-own the fd.
    let second = TcpEchoServer::new(
        TcpConfig {
            bind_strategy: Some(BindStrategy::Inherit(fd)),
            ..Default::default()
        }
        .into(),
    );
    let err = second.bind().await.err().expect("fd was already consumed");
    assert!(matches!(err, EchoError::FdInheritance(_)), "{err:?}");

    first.stop().await;
}

#[tokio::test]
async fn inherit_or_bind_prefers_fd_then_falls_back() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let inherited_addr = listener.local_addr().unwrap();
    let fd = InheritedFd::from(OwnedFd::from(listener));
    let strategy = BindStrategy::InheritOrBind {
        fd: Some(fd),
        fallback_target: Some(BindTarget::Network("127.0.0.1:0".parse().unwrap())),
    };

    let with_fd = start_tcp(TcpConfig {
        bind_strategy: Some(strategy.clone()),
        ..Default::default()
    })
    .await;
    assert_eq!(with_fd.addr, inherited_addr);

    // Same strategy again: the fd is consumed, so it binds the fallback.
    let fallback = start_tcp(TcpConfig {
        bind_strategy: Some(strategy),
        ..Default::default()
    })
    .await;
    assert_ne!(fallback.addr, inherited_addr);
    let mut client = TcpEchoClient::connect(fallback.addr).await.unwrap();
    assert_eq!(client.echo_string("fallback").await.unwrap(), "fallback");
    drop(client);

    with_fd.stop().await;
    fallback.stop().await;
}

#[tokio::test]
async fn with_fd_inheritance_binds_when_nothing_is_inherited() {
    // No LISTEN_* environment in the test process: falls back to bind_addr.
    let server = start_tcp(TcpConfig::default().with_fd_inheritance("tcp")).await;
    let mut client = TcpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(client.echo_string("fallback").await.unwrap(), "fallback");
    drop(client);
    server.stop().await;
}

/// `bind_addr` changed after `with_fd_inheritance` is the address bound.
#[tokio::test]
async fn with_fd_inheritance_binds_bind_addr_set_afterwards() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let wanted: SocketAddr = ([127, 0, 0, 1], port).into();

    let mut config = TcpConfig::default().with_fd_inheritance("tcp");
    config.bind_addr = wanted;
    let server = start_tcp(config).await;
    assert_eq!(server.addr, wanted);
    server.stop().await;

    let mut config = UdpConfig::default().with_fd_inheritance("udp");
    config.bind_addr = wanted;
    let server = start_udp(config).await;
    assert_eq!(server.addr, wanted);
    server.stop().await;

    let mut config = HttpConfig::default().with_fd_inheritance("http");
    config.bind_addr = wanted;
    let server = start_http(config).await;
    assert_eq!(server.addr, wanted);
    server.stop().await;
}

#[tokio::test]
async fn wrong_socket_kinds_are_rejected() {
    // UDP socket given to a TCP server.
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = TcpEchoServer::new(
        TcpConfig {
            bind_strategy: inherit(udp),
            ..Default::default()
        }
        .into(),
    );
    assert!(matches!(
        server.bind().await,
        Err(EchoError::FdInheritance(_))
    ));

    // TCP listener given to a UDP server.
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = UdpEchoServer::new(
        UdpConfig {
            bind_strategy: inherit(tcp),
            ..Default::default()
        }
        .into(),
    );
    assert!(matches!(
        server.bind().await,
        Err(EchoError::FdInheritance(_))
    ));

    // A connected (non-listening) TCP socket given to a TCP server. Detected
    // via SO_ACCEPTCONN, which macOS/BSD do not support (the check is skipped).
    #[cfg(target_os = "linux")]
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let server = TcpEchoServer::new(
            TcpConfig {
                bind_strategy: inherit(stream),
                ..Default::default()
            }
            .into(),
        );
        assert!(matches!(
            server.bind().await,
            Err(EchoError::FdInheritance(_))
        ));
    }

    // A Unix listener given to a TCP server (wrong family).
    let dir = socket_dir();
    let unix = std::os::unix::net::UnixListener::bind(dir.path().join("u.sock")).unwrap();
    let server = TcpEchoServer::new(
        TcpConfig {
            bind_strategy: inherit(unix),
            ..Default::default()
        }
        .into(),
    );
    assert!(matches!(
        server.bind().await,
        Err(EchoError::FdInheritance(_))
    ));

    // A TCP listener given to a Unix stream server (wrong family).
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = UnixStreamEchoServer::new(UnixStreamConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::from(OwnedFd::from(tcp))),
        ..Default::default()
    });
    assert!(matches!(
        server.bind().await,
        Err(EchoError::FdInheritance(_))
    ));

    // A regular file.
    let file = tempfile::tempfile().unwrap();
    let server = TcpEchoServer::new(
        TcpConfig {
            bind_strategy: inherit(file),
            ..Default::default()
        }
        .into(),
    );
    assert!(matches!(
        server.bind().await,
        Err(EchoError::FdInheritance(_))
    ));
}
