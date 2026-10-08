//! Unix domain socket (stream and datagram) integration tests.

mod common;

use common::{
    WAIT, payload, socket_dir, start_unix_datagram, start_unix_datagram_at, start_unix_stream,
    start_unix_stream_at, tagged_payload, try_start_unix_datagram, try_start_unix_stream,
};
use echosrv::datagram::DatagramClientConfig;
use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
use echosrv::{
    EchoClient, EchoError, EchoServerTrait, UnixDatagramEchoClient, UnixStreamEchoClient,
    UnixStreamEchoServer,
};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Reads until EOF or error; returns the bytes read before that.
async fn read_until_closed(stream: &mut UnixStream) -> Vec<u8> {
    let mut data = Vec::new();
    let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut data))
        .await
        .expect("connection was not closed by the server");
    data
}

/// Connects and echoes `msg`; `None` if the server closed the connection
/// instead of echoing.
async fn try_raw_echo(path: &Path, msg: &[u8]) -> Option<Vec<u8>> {
    let mut stream = UnixStream::connect(path).await.ok()?;
    stream.write_all(msg).await.ok()?;
    let mut buf = vec![0; msg.len()];
    match tokio::time::timeout(WAIT, stream.read_exact(&mut buf)).await {
        Ok(Ok(_)) => Some(buf),
        Ok(Err(_)) => None,
        Err(_) => panic!("server neither echoed nor closed the connection"),
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_echoes_text_and_binary() {
    let dir = socket_dir();
    let server = start_unix_stream_at(&dir.path().join("s.sock")).await;
    let mut client = UnixStreamEchoClient::connect(server.addr.clone())
        .await
        .unwrap();

    assert_eq!(
        client.echo_string("hello unix").await.unwrap(),
        "hello unix"
    );
    for data in [
        vec![0, 1, 2, 0, 255],
        (0..=255).collect::<Vec<u8>>(),
        payload(16 * 1024),
    ] {
        assert_eq!(client.echo(&data).await.unwrap(), data);
    }
    drop(client);
    server.stop().await;
}

/// Payloads larger than the socket buffers must not deadlock: the client
/// reads the echo while it is still writing.
#[tokio::test]
async fn stream_client_echoes_eight_mebibyte_payload() {
    let dir = socket_dir();
    let server = start_unix_stream(UnixStreamConfig {
        buffer_size: 64 * 1024,
        ..UnixStreamConfig::default().with_socket_path(dir.path().join("big.sock"))
    })
    .await;
    let config = echosrv::stream::ClientConfig {
        read_timeout: WAIT,
        write_timeout: WAIT,
        ..Default::default()
    };
    let mut client = UnixStreamEchoClient::connect_with_config(server.addr.clone(), config)
        .await
        .unwrap();
    let data = payload(8 * 1024 * 1024);
    let echoed = client.echo(&data).await.unwrap();
    assert!(echoed == data, "8 MiB payload was not echoed intact");
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn stream_concurrent_clients_get_their_own_payload() {
    let dir = socket_dir();
    let server = start_unix_stream_at(&dir.path().join("s.sock")).await;

    let tasks: Vec<_> = (0..20)
        .map(|id| {
            let path = server.addr.clone();
            tokio::spawn(async move {
                let mut client = UnixStreamEchoClient::connect(path).await?;
                for round in 0..3 {
                    let data = tagged_payload(id * 10 + round, 256);
                    assert_eq!(client.echo(&data).await?, data);
                }
                Ok::<(), EchoError>(())
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    server.stop().await;
}

#[tokio::test]
async fn stream_connection_limit_rejects_extra_and_frees_slot() {
    let dir = socket_dir();
    let server = start_unix_stream(UnixStreamConfig {
        max_connections: 2,
        ..UnixStreamConfig::default().with_socket_path(dir.path().join("s.sock"))
    })
    .await;
    let path = server.addr.clone();

    let mut first = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    let mut second = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(first.echo_string("one").await.unwrap(), "one");
    assert_eq!(second.echo_string("two").await.unwrap(), "two");

    let mut third = UnixStream::connect(&path).await.unwrap();
    let _ = third.write_all(b"rejected").await;
    assert!(
        read_until_closed(&mut third).await.is_empty(),
        "over-limit connection must not be echoed"
    );
    assert_eq!(server.stats.rejected_over_capacity(), 1);
    assert_eq!(second.echo_string("still").await.unwrap(), "still");

    drop(first);
    let mut attempts = 0;
    loop {
        if let Some(echoed) = try_raw_echo(&path, b"after release").await {
            assert_eq!(echoed, b"after release");
            break;
        }
        attempts += 1;
        assert!(attempts < 100, "slot was not released after disconnect");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(second);
    server.stop().await;
}

#[tokio::test]
async fn stream_socket_file_removed_on_shutdown_and_path_reusable() {
    let dir = socket_dir();
    let path = dir.path().join("s.sock");

    let server = start_unix_stream_at(&path).await;
    assert!(path.exists());
    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(client.echo_string("first run").await.unwrap(), "first run");
    drop(client);
    server.stop().await;
    assert!(!path.exists(), "socket file must be removed on shutdown");

    // Restart on the same path.
    let server = start_unix_stream_at(&path).await;
    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(
        client.echo_string("second run").await.unwrap(),
        "second run"
    );
    drop(client);
    server.stop().await;
    assert!(!path.exists());
}

#[tokio::test]
async fn stream_graceful_shutdown_closes_connected_clients() {
    let dir = socket_dir();
    let server = start_unix_stream_at(&dir.path().join("s.sock")).await;

    let mut stream = UnixStream::connect(&server.addr).await.unwrap();
    stream.write_all(b"hi").await.unwrap();
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf).await.unwrap();

    server.stop().await;
    assert!(read_until_closed(&mut stream).await.is_empty());
}

#[tokio::test]
async fn stream_stale_socket_file_is_recovered() {
    let dir = socket_dir();
    let path = dir.path().join("stale.sock");
    // Simulate a crashed server: socket file left behind, nobody listening.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());

    let server = start_unix_stream_at(&path).await;
    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(client.echo_string("recovered").await.unwrap(), "recovered");
    drop(client);
    server.stop().await;
    assert!(!path.exists());
}

#[tokio::test]
async fn stream_live_socket_is_not_hijacked() {
    let dir = socket_dir();
    let path = dir.path().join("live.sock");
    let server = start_unix_stream_at(&path).await;

    let err = try_start_unix_stream(UnixStreamConfig::default().with_socket_path(path.clone()))
        .await
        .err()
        .expect("binding a live socket path must fail");
    assert!(matches!(err, EchoError::Unix(_)), "{err:?}");

    // The original server is unaffected (and its file was not removed).
    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(
        client.echo_string("still mine").await.unwrap(),
        "still mine"
    );
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn stream_regular_file_at_path_is_not_removed() {
    let dir = socket_dir();
    let path = dir.path().join("file.sock");
    std::fs::write(&path, b"precious").unwrap();

    let err = try_start_unix_stream(UnixStreamConfig::default().with_socket_path(path.clone()))
        .await
        .err()
        .expect("binding over a regular file must fail");
    assert!(matches!(err, EchoError::Unix(_)), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), b"precious");
}

#[tokio::test]
async fn stream_creates_missing_parent_directory() {
    let dir = socket_dir();
    let path = dir.path().join("nested/dir/s.sock");
    let server = start_unix_stream_at(&path).await;
    let mut client = UnixStreamEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(client.echo_string("nested").await.unwrap(), "nested");
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn stream_run_honors_early_shutdown_and_cleans_up() {
    let dir = socket_dir();
    let path = dir.path().join("s.sock");
    let server =
        UnixStreamEchoServer::new(UnixStreamConfig::default().with_socket_path(path.clone()));
    server.shutdown_signal().send(()).unwrap();
    tokio::time::timeout(WAIT, server.run())
        .await
        .expect("run ignored an early shutdown")
        .unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn stream_client_connect_to_missing_socket_fails() {
    let dir = socket_dir();
    let err = UnixStreamEchoClient::connect(dir.path().join("nobody.sock"))
        .await
        .err()
        .expect("connect must fail");
    assert!(matches!(err, EchoError::Unix(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Datagram
// ---------------------------------------------------------------------------

#[tokio::test]
async fn datagram_echoes_text_and_binary() {
    let dir = socket_dir();
    let server = start_unix_datagram_at(&dir.path().join("d.sock")).await;
    let mut client = UnixDatagramEchoClient::connect(server.addr.clone())
        .await
        .unwrap();

    assert_eq!(
        client.echo_string("hello dgram").await.unwrap(),
        "hello dgram"
    );
    for data in [vec![0, 1, 0, 255], payload(2048)] {
        assert_eq!(client.echo(&data).await.unwrap(), data);
    }
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn datagram_concurrent_clients_get_their_own_payload() {
    let dir = socket_dir();
    let server = start_unix_datagram_at(&dir.path().join("d.sock")).await;

    let tasks: Vec<_> = (0..10)
        .map(|id| {
            let path = server.addr.clone();
            tokio::spawn(async move {
                let mut client = UnixDatagramEchoClient::connect(path).await?;
                for round in 0..5 {
                    let data = tagged_payload(id * 10 + round, 128);
                    assert_eq!(client.echo(&data).await?, data);
                }
                Ok::<(), EchoError>(())
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    server.stop().await;
}

#[tokio::test]
async fn datagram_client_temp_socket_removed_on_drop() {
    let dir = socket_dir();
    let server = start_unix_datagram_at(&dir.path().join("d.sock")).await;
    let mut client = UnixDatagramEchoClient::connect(server.addr.clone())
        .await
        .unwrap();
    let client_path = client
        .local_path()
        .expect("client binds a temporary path")
        .to_path_buf();
    assert!(client_path.exists());
    assert_eq!(client.echo_string("x").await.unwrap(), "x");

    drop(client);
    assert!(
        !client_path.exists(),
        "client temp socket must be removed on drop"
    );
    server.stop().await;
}

#[tokio::test]
async fn datagram_socket_file_removed_on_shutdown_and_path_reusable() {
    let dir = socket_dir();
    let path = dir.path().join("d.sock");

    let server = start_unix_datagram_at(&path).await;
    assert!(path.exists());
    server.stop().await;
    assert!(!path.exists(), "socket file must be removed on shutdown");

    let server = start_unix_datagram_at(&path).await;
    let mut client = UnixDatagramEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(client.echo_string("again").await.unwrap(), "again");
    drop(client);
    server.stop().await;
    assert!(!path.exists());
}

#[tokio::test]
async fn datagram_stale_socket_file_is_recovered() {
    let dir = socket_dir();
    let path = dir.path().join("stale.sock");
    drop(std::os::unix::net::UnixDatagram::bind(&path).unwrap());
    assert!(path.exists());

    let server = start_unix_datagram_at(&path).await;
    let mut client = UnixDatagramEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(client.echo_string("recovered").await.unwrap(), "recovered");
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn datagram_live_socket_is_not_hijacked() {
    let dir = socket_dir();
    let path = dir.path().join("live.sock");
    let server = start_unix_datagram_at(&path).await;

    let err = try_start_unix_datagram(UnixDatagramConfig::default().with_socket_path(path.clone()))
        .await
        .err()
        .expect("binding a live socket path must fail");
    assert!(matches!(err, EchoError::Unix(_)), "{err:?}");

    let mut client = UnixDatagramEchoClient::connect(path.clone()).await.unwrap();
    assert_eq!(
        client.echo_string("still mine").await.unwrap(),
        "still mine"
    );
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn datagram_truncates_to_server_buffer() {
    let dir = socket_dir();
    let server = start_unix_datagram(UnixDatagramConfig {
        buffer_size: 8,
        ..UnixDatagramConfig::default().with_socket_path(dir.path().join("d.sock"))
    })
    .await;
    let mut client = UnixDatagramEchoClient::connect(server.addr.clone())
        .await
        .unwrap();
    assert_eq!(client.echo(b"0123456789abcdef").await.unwrap(), b"01234567");
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn datagram_client_times_out_without_server() {
    let dir = socket_dir();
    // A bound socket that never answers.
    let silent_path = dir.path().join("silent.sock");
    let _silent = std::os::unix::net::UnixDatagram::bind(&silent_path).unwrap();

    let mut client = UnixDatagramEchoClient::connect_with_config(
        silent_path,
        DatagramClientConfig {
            read_timeout: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let err = client.echo(b"anyone?").await.unwrap_err();
    assert!(matches!(err, EchoError::Timeout(_)), "{err:?}");
}

#[tokio::test]
async fn datagram_unnamed_sender_does_not_break_server() {
    let dir = socket_dir();
    let server = start_unix_datagram_at(&dir.path().join("d.sock")).await;

    // An unbound sender cannot be replied to; the server must log and continue.
    let unnamed = tokio::net::UnixDatagram::unbound().unwrap();
    unnamed
        .send_to(b"no reply possible", &server.addr)
        .await
        .unwrap();

    let mut client = UnixDatagramEchoClient::connect(server.addr.clone())
        .await
        .unwrap();
    assert_eq!(client.echo_string("after").await.unwrap(), "after");
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn datagram_client_ignores_datagrams_from_other_senders() {
    // A hand-rolled "server" that lets a third party reply first.
    let dir = socket_dir();
    let server_path = dir.path().join("d.sock");
    let intruder_path = dir.path().join("i.sock");
    let server = tokio::net::UnixDatagram::bind(&server_path).unwrap();
    let mut client = UnixDatagramEchoClient::connect(server_path).await.unwrap();
    let responder = tokio::spawn(async move {
        let mut buf = [0u8; 64];
        let (n, peer) = server.recv_from(&mut buf).await.unwrap();
        let peer = peer.as_pathname().unwrap().to_path_buf();
        let intruder = tokio::net::UnixDatagram::bind(&intruder_path).unwrap();
        intruder.send_to(b"intruder", &peer).await.unwrap();
        let unnamed = tokio::net::UnixDatagram::unbound().unwrap();
        unnamed.send_to(b"unnamed", &peer).await.unwrap();
        server.send_to(&buf[..n], &peer).await.unwrap();
    });
    assert_eq!(client.echo(b"genuine").await.unwrap(), b"genuine");
    tokio::time::timeout(WAIT, responder)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn datagram_client_times_out_when_only_others_reply() {
    let dir = socket_dir();
    let server_path = dir.path().join("d.sock");
    let intruder_path = dir.path().join("i.sock");
    let server = tokio::net::UnixDatagram::bind(&server_path).unwrap();
    let mut client = UnixDatagramEchoClient::connect_with_config(
        server_path,
        DatagramClientConfig {
            read_timeout: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let responder = tokio::spawn(async move {
        let mut buf = [0u8; 64];
        let (_, peer) = server.recv_from(&mut buf).await.unwrap();
        let peer = peer.as_pathname().unwrap().to_path_buf();
        let intruder = tokio::net::UnixDatagram::bind(&intruder_path).unwrap();
        intruder.send_to(b"intruder", &peer).await.unwrap();
        server // keep the server socket open, silent
    });
    let err = client.echo(b"anyone?").await.unwrap_err();
    assert!(matches!(err, EchoError::Timeout(_)), "{err:?}");
    tokio::time::timeout(WAIT, responder)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn datagram_client_accepts_replies_via_symlinked_path() {
    // The server reports its bound path, not the symlink the client used.
    let dir = socket_dir();
    let server = start_unix_datagram_at(&dir.path().join("d.sock")).await;
    let link = dir.path().join("link.sock");
    std::os::unix::fs::symlink(&server.addr, &link).unwrap();

    let mut client = UnixDatagramEchoClient::connect(link).await.unwrap();
    assert_eq!(client.echo_string("via link").await.unwrap(), "via link");
    drop(client);
    server.stop().await;
}
