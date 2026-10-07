//! TCP echo server and stream client integration tests.

mod common;

use common::{WAIT, payload, start_tcp, start_tcp_with_limit, tagged_payload};
use echosrv::stream::{ClientConfig, ClientConfigBuilder};
use echosrv::{EchoClient, EchoError, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Reads until EOF or error and returns what was read before it. A reset
/// counts as "closed" too.
async fn read_until_closed(stream: &mut TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut data))
        .await
        .expect("connection was not closed by the server");
    data
}

/// Connects and echoes `msg`, returning `None` if the server closed the
/// connection instead of echoing (e.g. because the connection limit was hit).
async fn try_raw_echo(addr: SocketAddr, msg: &[u8]) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect(addr).await.ok()?;
    stream.write_all(msg).await.ok()?;
    let mut buf = vec![0; msg.len()];
    match tokio::time::timeout(WAIT, stream.read_exact(&mut buf)).await {
        Ok(Ok(_)) => Some(buf),
        Ok(Err(_)) => None,
        Err(_) => panic!("server neither echoed nor closed the connection"),
    }
}

#[tokio::test]
async fn echoes_text_and_binary_on_one_connection() {
    let server = start_tcp_with_limit(10).await;
    let mut client = TcpEchoClient::connect(server.addr).await.unwrap();

    assert_eq!(client.echo_string("hello").await.unwrap(), "hello");
    assert_eq!(
        client.echo_string("héllo wörld ✓").await.unwrap(),
        "héllo wörld ✓"
    );
    for data in [
        vec![0, 1, 2, 3, 0, 255, 128, 0],
        vec![0; 100],
        vec![255; 100],
        (0..=255).collect::<Vec<u8>>(),
        payload(8192),
    ] {
        assert_eq!(client.echo(&data).await.unwrap(), data);
    }
    // Empty input short-circuits without touching the socket.
    assert!(client.echo(b"").await.unwrap().is_empty());

    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn echoes_one_mebibyte_payload() {
    let server = start_tcp(TcpConfig {
        buffer_size: 64 * 1024,
        ..Default::default()
    })
    .await;
    let data = payload(1024 * 1024);

    // Write and read concurrently: a 1 MiB echo exceeds the socket buffers,
    // so writing everything before reading would deadlock.
    let stream = TcpStream::connect(server.addr).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let to_send = data.clone();
    let writer_task = tokio::spawn(async move {
        writer.write_all(&to_send).await.unwrap();
        writer.shutdown().await.unwrap();
    });
    let mut echoed = Vec::with_capacity(data.len());
    tokio::time::timeout(WAIT, reader.read_to_end(&mut echoed))
        .await
        .expect("1 MiB echo timed out")
        .unwrap();
    writer_task.await.unwrap();

    assert_eq!(echoed.len(), data.len());
    assert!(echoed == data, "1 MiB echo differs from input");
    server.stop().await;
}

/// `Client::echo` writes the whole request before reading, so payloads larger
/// than the loopback socket buffers can deadlock against the echo server.
#[tokio::test]
#[ignore = "BUG: stream Client::echo writes the full payload before reading the echo; payloads larger than the socket buffers (8 MiB on macOS loopback) deadlock until the write timeout"]
async fn client_echoes_eight_mebibyte_payload() {
    let server = start_tcp(TcpConfig {
        buffer_size: 64 * 1024,
        ..Default::default()
    })
    .await;
    let data = payload(8 * 1024 * 1024);
    let config = ClientConfig {
        read_timeout: WAIT,
        write_timeout: WAIT,
        ..Default::default()
    };
    let mut client = TcpEchoClient::connect_with_config(server.addr, config)
        .await
        .unwrap();
    let echoed = client.echo(&data).await.unwrap();
    assert!(echoed == data);
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn many_concurrent_clients_get_their_own_payload() {
    let server = start_tcp_with_limit(100).await;
    let addr = server.addr;

    let tasks: Vec<_> = (0..50)
        .map(|id| {
            tokio::spawn(async move {
                let mut client = TcpEchoClient::connect(addr).await?;
                for round in 0..3 {
                    let data = tagged_payload(id * 10 + round, 512 + id);
                    let echoed = client.echo(&data).await?;
                    assert_eq!(echoed, data, "client {id} round {round}");
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
async fn sequential_connect_disconnect_cycles() {
    let server = start_tcp_with_limit(1).await;
    // With a limit of one, every cycle only works if the previous connection's
    // slot was released; retry briefly because release happens asynchronously
    // after the server observes EOF.
    for i in 0..20 {
        let msg = format!("cycle {i}");
        let mut attempts = 0;
        loop {
            if let Some(echoed) = try_raw_echo(server.addr, msg.as_bytes()).await {
                assert_eq!(echoed, msg.as_bytes());
                break;
            }
            attempts += 1;
            assert!(attempts < 100, "connection slot was never released");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    server.stop().await;
}

#[tokio::test]
async fn connection_limit_rejects_extra_and_frees_slot() {
    let server = start_tcp_with_limit(2).await;

    // Hold two connections open; echoing proves the server accepted (and
    // counted) both.
    let mut first = TcpEchoClient::connect(server.addr).await.unwrap();
    let mut second = TcpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(first.echo_string("one").await.unwrap(), "one");
    assert_eq!(second.echo_string("two").await.unwrap(), "two");

    // A third connection is accepted by the kernel but closed by the server
    // without echoing.
    let mut third = TcpStream::connect(server.addr).await.unwrap();
    let _ = third.write_all(b"rejected").await;
    assert!(
        read_until_closed(&mut third).await.is_empty(),
        "over-limit connection must not be echoed"
    );

    // The held connections are unaffected.
    assert_eq!(first.echo_string("still").await.unwrap(), "still");

    // Releasing one frees a slot (asynchronously, once the server sees EOF).
    drop(first);
    let mut attempts = 0;
    loop {
        if let Some(echoed) = try_raw_echo(server.addr, b"after release").await {
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
async fn idle_client_is_disconnected_after_read_timeout() {
    let read_timeout = Duration::from_millis(200);
    let server = start_tcp(TcpConfig {
        read_timeout,
        ..Default::default()
    })
    .await;

    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");

    // Now stay idle: the server must close the connection after ~200 ms.
    let idle_since = Instant::now();
    assert!(read_until_closed(&mut stream).await.is_empty());
    let elapsed = idle_since.elapsed();
    assert!(
        elapsed >= read_timeout - Duration::from_millis(20),
        "closed too early ({elapsed:?})"
    );
    assert!(elapsed < WAIT, "closed too late ({elapsed:?})");

    server.stop().await;
}

#[tokio::test]
async fn graceful_shutdown_closes_connected_clients() {
    let server = start_tcp_with_limit(10).await;
    let addr = server.addr;

    let mut clients = Vec::new();
    for i in 0..3 {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let msg = format!("client {i}");
        stream.write_all(msg.as_bytes()).await.unwrap();
        let mut buf = vec![0; msg.len()];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, msg.as_bytes());
        clients.push(stream);
    }

    // serve() must return Ok even with live connections ...
    server.stop().await;

    // ... every client sees the connection closed ...
    for stream in &mut clients {
        assert!(read_until_closed(stream).await.is_empty());
    }

    // ... and the listener is gone.
    let err = TcpEchoClient::connect(addr)
        .await
        .err()
        .expect("connect after shutdown must fail");
    assert!(
        matches!(err, EchoError::Tcp(ref e) if e.kind() == std::io::ErrorKind::ConnectionRefused),
        "{err:?}"
    );
}

#[tokio::test]
async fn shutdown_requested_before_serve_is_honored() {
    let server = TcpEchoServer::new(TcpConfig::default().into());
    let shutdown = server.shutdown_signal();
    let bound = server.bind().await.unwrap();
    shutdown.send(()).unwrap();
    tokio::time::timeout(WAIT, bound.serve())
        .await
        .expect("serve ignored an early shutdown")
        .unwrap();
}

#[tokio::test]
async fn run_returns_ok_after_shutdown() {
    let server = TcpEchoServer::new(TcpConfig::default().into());
    let shutdown = server.shutdown_signal();
    // Sent before run() subscribes; must not be lost.
    shutdown.send(()).unwrap();
    tokio::time::timeout(WAIT, server.run())
        .await
        .expect("run ignored the shutdown")
        .unwrap();
}

#[tokio::test]
async fn invalid_config_is_rejected_at_bind() {
    for config in [
        TcpConfig {
            buffer_size: 0,
            ..Default::default()
        },
        TcpConfig {
            max_connections: 0,
            ..Default::default()
        },
    ] {
        let server = TcpEchoServer::new(config.into());
        assert!(matches!(server.bind().await, Err(EchoError::Config(_))));
    }
}

#[tokio::test]
async fn binding_an_address_in_use_fails() {
    let server = start_tcp_with_limit(1).await;
    let second = TcpEchoServer::new(
        TcpConfig {
            bind_addr: server.addr,
            ..Default::default()
        }
        .into(),
    );
    let err = second.bind().await.err().expect("second bind must fail");
    assert!(
        matches!(err, EchoError::Tcp(ref e) if e.kind() == std::io::ErrorKind::AddrInUse),
        "{err:?}"
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Client behavior
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_rejects_response_larger_than_max_response_size() {
    // A misbehaving "echo" server that answers with far more than it got.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let fake = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 100];
        stream.read_exact(&mut buf).await.unwrap();
        stream.write_all(&[b'x'; 4096]).await.unwrap();
        // Keep the connection open until the client gives up.
        let _ = stream.read(&mut buf).await;
    });

    let config = ClientConfigBuilder::new()
        .max_response_size(100)
        .buffer_size(8192)
        .build();
    let mut client = TcpEchoClient::connect_with_config(addr, config)
        .await
        .unwrap();
    let err = client
        .echo(&[b'a'; 100])
        .await
        .expect_err("oversized response must be an error");
    assert!(
        matches!(err, EchoError::Config(ref msg) if msg.contains("Response too large")),
        "{err:?}"
    );

    drop(client);
    fake.await.unwrap();
}

#[tokio::test]
async fn client_rejects_request_larger_than_max_response_size() {
    let server = start_tcp_with_limit(1).await;
    let config = ClientConfigBuilder::new().max_response_size(16).build();
    let mut client = TcpEchoClient::connect_with_config(server.addr, config)
        .await
        .unwrap();
    let err = client.echo(&[0u8; 17]).await.unwrap_err();
    assert!(
        matches!(err, EchoError::Config(ref msg) if msg.contains("Request too large")),
        "{err:?}"
    );
    // Exactly at the limit is fine.
    assert_eq!(client.echo(&[7u8; 16]).await.unwrap(), [7u8; 16]);
    drop(client);
    server.stop().await;
}

#[tokio::test]
async fn client_connect_refused_is_an_error() {
    // Shut a server down to get an address nobody listens on.
    let addr = start_tcp_with_limit(1).await.stop().await;
    let err = TcpEchoClient::connect(addr).await.err().expect("must fail");
    assert!(
        matches!(err, EchoError::Tcp(ref e) if e.kind() == std::io::ErrorKind::ConnectionRefused),
        "{err:?}"
    );
}

#[tokio::test]
async fn client_connect_times_out() {
    // A listener with a tiny backlog that never accepts: once its queue is
    // full, the kernel drops further SYNs and connects hang.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    let _listener = socket.listen(1).unwrap();

    let mut held = Vec::new();
    let mut saturated = false;
    for _ in 0..64 {
        match tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => held.push(stream),
            Ok(Err(e)) => panic!("unexpected connect error while filling backlog: {e}"),
            Err(_) => {
                saturated = true;
                break;
            }
        }
    }
    if !saturated {
        eprintln!("skipping: could not saturate the listen backlog on this platform");
        return;
    }

    let config = ClientConfigBuilder::new()
        .connect_timeout(Duration::from_millis(200))
        .build();
    let started = Instant::now();
    let err = TcpEchoClient::connect_with_config(addr, config)
        .await
        .err()
        .expect("connect must time out");
    assert!(matches!(err, EchoError::Timeout(_)), "{err:?}");
    assert!(started.elapsed() < WAIT);
}

#[tokio::test]
async fn client_read_timeout_when_server_does_not_answer() {
    // Accepts but never replies.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let silent = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf).await;
    });

    let config = ClientConfigBuilder::new()
        .read_timeout(Duration::from_millis(100))
        .build();
    let mut client = TcpEchoClient::connect_with_config(addr, config)
        .await
        .unwrap();
    let err = client.echo(b"hello?").await.unwrap_err();
    assert!(matches!(err, EchoError::Timeout(_)), "{err:?}");

    drop(client);
    silent.await.unwrap();
}

#[tokio::test]
async fn client_tracks_idle_time() {
    let server = start_tcp_with_limit(1).await;
    let mut client = TcpEchoClient::connect(server.addr).await.unwrap();
    assert!(!client.is_idle(Duration::from_secs(60)));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(client.is_idle(Duration::from_millis(10)));
    client.echo(b"activity").await.unwrap();
    assert!(!client.is_idle(Duration::from_millis(10)));
    drop(client);
    server.stop().await;
}
