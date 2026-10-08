use crate::EchoError;
use crate::common::{EchoClient, EchoServerTrait};
use crate::datagram::{DatagramClientConfig, DatagramConfig, DatagramEchoServer, DatagramProtocol};
use crate::network::{Address, BindStrategy, FdInheritanceConfig, InheritedFd};
use crate::stream::{StreamConfig, StreamEchoServer, StreamProtocol};
use crate::unix::{
    UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixDatagramExt,
    UnixDatagramProtocol, UnixStreamConfig, UnixStreamEchoClient, UnixStreamEchoServer,
    UnixStreamExt, UnixStreamProtocol,
};
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::{TempDir, tempdir};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// A running Unix stream server bound to a unique temporary path.
struct StreamServer {
    _dir: TempDir,
    path: PathBuf,
    shutdown: broadcast::Sender<()>,
    handle: JoinHandle<crate::Result<()>>,
}

impl StreamServer {
    async fn start(configure: impl FnOnce(UnixStreamConfig) -> UnixStreamConfig) -> Self {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stream.sock");
        let config = configure(UnixStreamConfig::default().with_socket_path(path.clone()));
        let server = UnixStreamEchoServer::new(config);
        let shutdown = server.shutdown_signal();
        // bind() returns once the socket is listening: no sleep needed.
        let bound = server.bind().await.unwrap();
        assert_eq!(bound.local_addr().as_unix(), Some(&path));
        let handle = tokio::spawn(bound.serve());
        Self {
            _dir: dir,
            path,
            shutdown,
            handle,
        }
    }

    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        self.handle.await.unwrap().unwrap();
        assert!(!self.path.exists(), "socket file not cleaned up");
    }
}

/// A running Unix datagram server bound to a unique temporary path.
struct DatagramServer {
    _dir: TempDir,
    path: PathBuf,
    shutdown: broadcast::Sender<()>,
    handle: JoinHandle<crate::Result<()>>,
}

impl DatagramServer {
    async fn start() -> Self {
        let dir = tempdir().unwrap();
        let path = dir.path().join("dgram.sock");
        let server = UnixDatagramEchoServer::new(
            UnixDatagramConfig::default().with_socket_path(path.clone()),
        );
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.unwrap();
        assert_eq!(bound.local_addr().as_unix(), Some(&path));
        let handle = tokio::spawn(bound.serve());
        Self {
            _dir: dir,
            path,
            shutdown,
            handle,
        }
    }

    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        self.handle.await.unwrap().unwrap();
        assert!(!self.path.exists(), "socket file not cleaned up");
    }
}

// --- Unix stream server ------------------------------------------------------

#[tokio::test]
async fn test_unix_stream_echo() {
    let server = StreamServer::start(|c| c).await;
    let mut client = UnixStreamEchoClient::connect(server.path.clone())
        .await
        .unwrap();

    assert_eq!(
        client.echo_string("Hello, Unix Stream!").await.unwrap(),
        "Hello, Unix Stream!"
    );
    let binary = b"Binary data with \x00 null bytes";
    assert_eq!(client.echo(binary).await.unwrap(), binary);

    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn test_unix_stream_multiple_clients() {
    let server = StreamServer::start(|c| c).await;
    let mut handles = Vec::new();
    for i in 0..5 {
        let path = server.path.clone();
        handles.push(tokio::spawn(async move {
            let mut client = UnixStreamEchoClient::connect(path).await.unwrap();
            let message = format!("Client {i}");
            assert_eq!(client.echo_string(&message).await.unwrap(), message);
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    server.stop().await;
}

#[tokio::test]
async fn test_unix_stream_large_data() {
    let server = StreamServer::start(|c| c).await;
    let mut client = UnixStreamEchoClient::connect(server.path.clone())
        .await
        .unwrap();
    // Larger than the 1 KiB server buffer.
    let large: Vec<u8> = (0..5000).map(|i| (i % 256) as u8).collect();
    assert_eq!(client.echo(&large).await.unwrap(), large);
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn test_unix_stream_shutdown_with_idle_client_connected() {
    let server = StreamServer::start(|c| c).await;
    let mut client = UnixStreamEchoClient::connect(server.path.clone())
        .await
        .unwrap();
    assert_eq!(client.echo_string("x").await.unwrap(), "x");
    // stop() must not hang on the idle connection.
    tokio::time::timeout(Duration::from_secs(5), server.stop())
        .await
        .expect("shutdown hung on an open connection");
}

#[tokio::test]
async fn test_unix_stream_idle_connection_closed_after_read_timeout() {
    use tokio::io::AsyncReadExt;

    let server = StreamServer::start(|c| UnixStreamConfig {
        read_timeout: Duration::from_millis(50),
        ..c
    })
    .await;
    let mut raw = tokio::net::UnixStream::connect(&server.path).await.unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(5), raw.read(&mut buf))
        .await
        .expect("server did not close the idle connection")
        .unwrap();
    assert_eq!(n, 0);
    server.stop().await;
}

#[tokio::test]
async fn test_socket_file_removed_on_shutdown_and_restart_works() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("restart.sock");

    for _ in 0..2 {
        let server = UnixStreamEchoServer::new(
            UnixStreamConfig::default().with_socket_path(socket_path.clone()),
        );
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.unwrap();
        assert_eq!(bound.local_addr().as_unix(), Some(&socket_path));
        let handle = tokio::spawn(bound.serve());

        let mut client = UnixStreamEchoClient::connect(socket_path.clone())
            .await
            .unwrap();
        assert_eq!(client.echo_string("again").await.unwrap(), "again");

        shutdown.send(()).unwrap();
        handle.await.unwrap().unwrap();
        assert!(!socket_path.exists(), "socket file not cleaned up");
    }
}

#[tokio::test]
async fn test_run_honors_shutdown_sent_before_run() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("early.sock");
    let server =
        UnixStreamEchoServer::new(UnixStreamConfig::default().with_socket_path(path.clone()));
    server.shutdown_signal().send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server.run())
        .await
        .expect("run() ignored an early shutdown")
        .unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn test_stale_socket_file_is_recovered() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&socket_path).unwrap());
    assert!(socket_path.exists());

    let server = UnixStreamEchoServer::new(
        UnixStreamConfig::default().with_socket_path(socket_path.clone()),
    );
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await.unwrap().serve());
    let mut client = UnixStreamEchoClient::connect(socket_path).await.unwrap();
    assert_eq!(client.echo_string("ok").await.unwrap(), "ok");
    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_live_socket_is_not_stolen() {
    let server = StreamServer::start(|c| c).await;
    let second = UnixStreamEchoServer::new(
        UnixStreamConfig::default().with_socket_path(server.path.clone()),
    );
    match second.bind().await {
        Err(EchoError::Unix(e)) => assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse),
        Err(other) => panic!("expected AddrInUse, got {other:?}"),
        Ok(_) => panic!("second server stole a live socket"),
    }
    // The first server still works.
    let mut client = UnixStreamEchoClient::connect(server.path.clone())
        .await
        .unwrap();
    assert_eq!(
        client.echo_string("still here").await.unwrap(),
        "still here"
    );
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn test_inherited_socket_file_is_not_removed() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("inherited.sock");
    let parent_listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

    let config = UnixStreamConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(parent_listener))),
        ..UnixStreamConfig::default()
    };
    let server = UnixStreamEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr().as_unix(), Some(&socket_path));
    let handle = tokio::spawn(bound.serve());

    let mut client = UnixStreamEchoClient::connect(socket_path.clone())
        .await
        .unwrap();
    assert_eq!(client.echo_string("inherited").await.unwrap(), "inherited");

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
    assert!(
        socket_path.exists(),
        "inherited socket file must not be removed"
    );
}

/// An inherited unnamed socket (here an unbound datagram socket) has no
/// address to report, but binding must still succeed.
#[tokio::test]
async fn test_inherited_unnamed_datagram_socket() {
    let unbound = std::os::unix::net::UnixDatagram::unbound().unwrap();
    let config = UnixDatagramConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(unbound))),
        ..UnixDatagramConfig::default()
    };
    let server = UnixDatagramEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr(), &Address::UnixUnnamed);
    let handle = tokio::spawn(bound.serve());

    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

/// A unique abstract socket name for this test process.
#[cfg(target_os = "linux")]
fn abstract_name(tag: &str) -> (Vec<u8>, std::os::unix::net::SocketAddr) {
    use std::os::linux::net::SocketAddrExt;
    let name = format!("echosrv-test-{tag}-{}", std::process::id()).into_bytes();
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).unwrap();
    (name, addr)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_inherited_abstract_stream_socket() {
    use crate::stream::Client;

    let (name, addr) = abstract_name("stream");
    let parent = std::os::unix::net::UnixListener::bind_addr(&addr).unwrap();
    let config = UnixStreamConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(parent))),
        ..UnixStreamConfig::default()
    };
    let server = UnixStreamEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    let local = bound.local_addr().clone();
    assert_eq!(local, Address::UnixAbstract(name.clone()));
    assert_eq!(
        local.to_string(),
        format!("unix:@{}", String::from_utf8(name).unwrap())
    );
    let handle = tokio::spawn(bound.serve());

    let mut client = Client::<UnixStreamProtocol>::connect(local).await.unwrap();
    assert_eq!(client.echo_string("abstract").await.unwrap(), "abstract");
    drop(client);

    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_inherited_abstract_datagram_socket() {
    let (name, addr) = abstract_name("dgram");
    let parent = std::os::unix::net::UnixDatagram::bind_addr(&addr).unwrap();
    let config = UnixDatagramConfig {
        bind_strategy: BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(parent))),
        ..UnixDatagramConfig::default()
    };
    let server = UnixDatagramEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr(), &Address::UnixAbstract(name));
    let handle = tokio::spawn(bound.serve());

    shutdown.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn test_connect_abstract_is_unsupported_off_linux() {
    let result = UnixStreamProtocol::connect_address(&Address::UnixAbstract(b"x".to_vec())).await;
    assert!(
        matches!(result, Err(EchoError::Unsupported(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn test_unix_stream_connection_limit() {
    let server = StreamServer::start(|c| UnixStreamConfig {
        max_connections: 1,
        ..c
    })
    .await;

    let mut first = UnixStreamEchoClient::connect(server.path.clone())
        .await
        .unwrap();
    assert_eq!(first.echo_string("one").await.unwrap(), "one");

    let mut second = UnixStreamEchoClient::connect(server.path.clone())
        .await
        .unwrap();
    let rejected = second.echo_string("two").await;
    assert!(!matches!(rejected, Ok(ref s) if s == "two"));

    drop(first);
    // The slot frees once the server notices the disconnect.
    let mut ok = false;
    for _ in 0..50 {
        let mut third = UnixStreamEchoClient::connect(server.path.clone())
            .await
            .unwrap();
        if matches!(third.echo_string("three").await, Ok(ref s) if s == "three") {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ok, "connection slot was not released");

    server.stop().await;
}

// --- Generic servers over the Unix protocols honor the config path -------------

#[tokio::test]
async fn test_generic_stream_server_honors_unix_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("generic-stream.sock");
    let config: StreamConfig = UnixStreamConfig::default()
        .with_socket_path(path.clone())
        .into();
    let server: StreamEchoServer<UnixStreamProtocol> = StreamEchoServer::new(config);
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr().as_unix(), Some(&path));
    let handle = tokio::spawn(bound.serve());

    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(client.echo_string("generic").await.unwrap(), "generic");

    drop(client);
    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn test_generic_datagram_server_honors_unix_config() {
    let temp_dir = tempdir().unwrap();
    let socket_path = temp_dir.path().join("generic.sock");
    let config = UnixDatagramConfig::default().with_socket_path(socket_path.clone());
    let server: DatagramEchoServer<UnixDatagramProtocol> = DatagramEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    assert_eq!(bound.local_addr().as_unix(), Some(&socket_path));
    let handle = tokio::spawn(bound.serve());

    let mut client = UnixDatagramEchoClient::connect(socket_path.clone())
        .await
        .unwrap();
    let client_path = client.local_path().unwrap().to_path_buf();
    assert!(client_path.exists());
    // macOS caps Unix datagrams at its default SO_SNDBUF (2 KiB).
    let payload = vec![7u8; 1500];
    assert_eq!(client.echo(&payload).await.unwrap(), payload);
    drop(client);
    assert!(
        !client_path.exists(),
        "client temp socket not removed on drop"
    );

    shutdown.send(()).unwrap();
    handle.await.unwrap().unwrap();
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn test_stream_protocol_bind_with_pool_uses_service_name() {
    let dir = tempdir().unwrap();
    let inherited_path = dir.path().join("pooled.sock");
    let fallback_path = dir.path().join("fallback.sock");
    let parent = std::os::unix::net::UnixListener::bind(&inherited_path).unwrap();
    let pool = FdInheritanceConfig::from_fds([("echo".to_string(), OwnedFd::from(parent))]);

    let config =
        UnixStreamConfig::default().with_fd_inheritance("echo".into(), fallback_path.clone());
    let listener = UnixStreamProtocol::bind_unix_with_inheritance(&config, &pool)
        .await
        .unwrap();
    assert!(listener.owned_socket_path().is_none());
    assert!(!fallback_path.exists());
    use crate::network::LocalAddress;
    assert_eq!(
        listener.local_address().unwrap(),
        Address::Unix(inherited_path.clone())
    );

    // Pool is drained: the next bind falls back to the path, and owns it.
    let listener2 = UnixStreamProtocol::bind_unix_with_inheritance(&config, &pool)
        .await
        .unwrap();
    assert_eq!(listener2.owned_socket_path(), Some(fallback_path.as_path()));
    drop(listener2);
    assert!(!fallback_path.exists());
    drop(listener);
    assert!(inherited_path.exists());
}

#[tokio::test]
async fn test_datagram_protocol_bind_with_pool_uses_service_name() {
    let dir = tempdir().unwrap();
    let inherited_path = dir.path().join("pooled-dgram.sock");
    let parent = std::os::unix::net::UnixDatagram::bind(&inherited_path).unwrap();
    let pool = FdInheritanceConfig::from_fds([("d".to_string(), OwnedFd::from(parent))]);

    let config =
        UnixDatagramConfig::default().with_fd_inheritance("d".into(), dir.path().join("fb.sock"));
    let socket = UnixDatagramProtocol::bind_unix_with_inheritance(&config, &pool)
        .await
        .unwrap();
    assert!(socket.owned_socket_path().is_none());
    drop(socket);
    assert!(inherited_path.exists());

    // Through the generic trait too.
    let dgram: DatagramConfig = config.into();
    let socket = <UnixDatagramProtocol as DatagramProtocol>::bind_with_inheritance(&dgram, &pool)
        .await
        .unwrap();
    assert!(socket.owned_socket_path().is_some());
}

// --- Protocol trait edge cases -------------------------------------------------

#[tokio::test]
async fn test_stream_protocol_rejects_network_addresses() {
    let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    assert!(matches!(
        UnixStreamProtocol::connect(addr).await,
        Err(EchoError::Unsupported(_))
    ));
    assert!(matches!(
        UnixStreamProtocol::connect_address(&Address::Network(addr)).await,
        Err(EchoError::Unsupported(_))
    ));
    let config = StreamConfig::default(); // no Unix strategy: binds a network addr
    assert!(matches!(
        UnixStreamProtocol::bind_with_inheritance(&config, &FdInheritanceConfig::empty()).await,
        Err(EchoError::Config(_))
    ));
}

#[tokio::test]
async fn test_stream_protocol_connect_missing_path_is_unix_error() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.sock");
    match UnixStreamProtocol::connect_unix(&missing).await {
        Err(EchoError::Unix(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected Unix(NotFound), got {other:?}"),
    }
    assert!(matches!(
        UnixStreamEchoClient::connect(missing).await,
        Err(EchoError::Unix(_))
    ));
}

#[tokio::test]
async fn test_datagram_send_to_unnamed_peer_fails() {
    let socket = UnixDatagramProtocol::create_client_socket().await.unwrap();
    match UnixDatagramProtocol::send_to(&socket, b"x", &None).await {
        Err(EchoError::Unix(e)) => assert_eq!(e.kind(), std::io::ErrorKind::AddrNotAvailable),
        other => panic!("expected Unix(AddrNotAvailable), got {other:?}"),
    }
}

#[tokio::test]
async fn test_datagram_server_replies_to_peer_path() {
    let server = DatagramServer::start().await;

    // Two independent named clients each get their own reply.
    let a = UnixDatagramProtocol::create_client_socket().await.unwrap();
    let b = UnixDatagramProtocol::create_client_socket().await.unwrap();
    assert_ne!(a.owned_socket_path(), b.owned_socket_path());
    a.get_ref().send_to(b"from a", &server.path).await.unwrap();
    b.get_ref().send_to(b"from b", &server.path).await.unwrap();

    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), a.get_ref().recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"from a");
    let n = tokio::time::timeout(Duration::from_secs(5), b.get_ref().recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"from b");

    // An unnamed sender cannot be replied to, and the server keeps running.
    let unnamed = tokio::net::UnixDatagram::unbound().unwrap();
    unnamed.send_to(b"lost", &server.path).await.unwrap();
    a.get_ref().send_to(b"again", &server.path).await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(5), a.get_ref().recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"again");

    server.stop().await;
}

#[tokio::test]
async fn test_datagram_connect_unix_uses_send_recv() {
    let server = DatagramServer::start().await;
    let client = UnixDatagramProtocol::connect_unix(&server.path)
        .await
        .unwrap();
    let client_path = client.owned_socket_path().unwrap().to_path_buf();
    client.get_ref().send(b"connected").await.unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), client.get_ref().recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"connected");
    drop(client);
    assert!(!client_path.exists());
    server.stop().await;
}

// --- Unix datagram server / client -----------------------------------------------

#[tokio::test]
async fn test_unix_datagram_echo() {
    let server = DatagramServer::start().await;
    let mut client = UnixDatagramEchoClient::connect(server.path.clone())
        .await
        .unwrap();
    assert_eq!(
        client.echo_string("Hello, Unix Datagram!").await.unwrap(),
        "Hello, Unix Datagram!"
    );
    let binary = b"Binary data with \x00 null bytes";
    assert_eq!(client.echo(binary).await.unwrap(), binary);
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn test_unix_datagram_client_zero_buffer_rejected() {
    let config = DatagramClientConfig {
        buffer_size: 0,
        ..Default::default()
    };
    let result =
        UnixDatagramEchoClient::connect_with_config("/nonexistent.sock".into(), config).await;
    assert!(matches!(result, Err(EchoError::Config(_))));
}

#[tokio::test]
async fn test_unix_datagram_client_times_out_without_reply() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("silent.sock");
    let _silent = tokio::net::UnixDatagram::bind(&path).unwrap();
    let mut client = UnixDatagramEchoClient::connect_with_config(
        path,
        DatagramClientConfig {
            read_timeout: Duration::from_secs(10),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    tokio::time::pause();
    match client.echo(b"anyone?").await {
        Err(EchoError::Timeout(msg)) => assert!(msg.contains("receive"), "{msg}"),
        other => panic!("expected Timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn test_unix_datagram_client_send_to_missing_server_fails() {
    let dir = tempdir().unwrap();
    let mut client = UnixDatagramEchoClient::connect(dir.path().join("missing.sock"))
        .await
        .unwrap();
    let err = client.echo(b"x").await.unwrap_err();
    assert!(matches!(err, EchoError::Unix(_)), "{err:?}");
    assert_eq!(err.io_error_kind(), Some(std::io::ErrorKind::NotFound));
}

#[tokio::test]
async fn test_unix_datagram_client_send_to_stale_socket_is_refused() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("stale.sock");
    // A socket file left behind with nobody bound to it any more.
    drop(std::os::unix::net::UnixDatagram::bind(&path).unwrap());
    assert!(path.exists());
    let mut client = UnixDatagramEchoClient::connect(path).await.unwrap();
    let err = client.echo(b"x").await.unwrap_err();
    assert_eq!(
        err.io_error_kind(),
        Some(std::io::ErrorKind::ConnectionRefused),
        "{err:?}"
    );
}

#[tokio::test]
async fn test_unix_datagram_client_rejects_reply_larger_than_buffer() {
    let server = DatagramServer::start().await;
    let config = DatagramClientConfig {
        buffer_size: 4,
        ..Default::default()
    };
    let mut client = UnixDatagramEchoClient::connect_with_config(server.path.clone(), config)
        .await
        .unwrap();
    match client.echo(b"12345").await {
        Err(EchoError::Config(msg)) => assert!(msg.contains("buffer_size of 4"), "{msg}"),
        other => panic!("expected Config error, got {other:?}"),
    }
    assert_eq!(client.echo(b"1234").await.unwrap(), b"1234");
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn test_unix_datagram_client_paths_are_unique_and_cleaned() {
    let mut paths = Vec::new();
    let mut clients = Vec::new();
    for _ in 0..10 {
        let client = UnixDatagramEchoClient::connect("/nonexistent.sock".into())
            .await
            .unwrap();
        let path = client.local_path().unwrap().to_path_buf();
        assert!(path.exists());
        assert!(!paths.contains(&path));
        paths.push(path);
        clients.push(client);
    }
    drop(clients);
    for path in paths {
        assert!(!path.exists(), "{} leaked", path.display());
    }
}

// --- Linux abstract namespace ---------------------------------------------------

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_abstract_namespace_sockets() {
    let name = format!(
        "echosrv-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );

    // Datagram: bind an abstract socket and talk to it from an unbound socket.
    let abstract_dgram = UnixDatagramProtocol::bind_abstract(&name).await.unwrap();
    let local = abstract_dgram.local_addr().unwrap();
    assert!(local.as_pathname().is_none(), "abstract socket has no path");

    // Stream: connect_abstract reaches a listener in the abstract namespace.
    use std::os::linux::net::SocketAddrExt;
    let stream_name = format!("{name}-stream");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(stream_name.as_bytes()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr).unwrap();
    let stream = UnixStreamProtocol::connect_abstract(&stream_name)
        .await
        .unwrap();
    let (_accepted, _) = listener.accept().unwrap();
    drop(stream);

    // Connecting to an unused abstract name is refused.
    assert!(
        UnixStreamProtocol::connect_abstract(&format!("{name}-unused"))
            .await
            .is_err()
    );
}
