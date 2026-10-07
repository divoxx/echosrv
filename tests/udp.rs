//! UDP echo server and datagram client integration tests (real `UdpEchoServer`).

mod common;

use common::{WAIT, payload, start_udp, tagged_payload, try_start_udp};
use echosrv::datagram::DatagramClientConfig;
use echosrv::{EchoClient, EchoError, EchoServerTrait, UdpConfig, UdpEchoClient, UdpEchoServer};
use std::time::Duration;

fn short_timeout_client() -> DatagramClientConfig {
    DatagramClientConfig {
        read_timeout: Duration::from_millis(200),
        ..Default::default()
    }
}

#[tokio::test]
async fn echoes_text_and_binary() {
    let server = start_udp(UdpConfig::default()).await;
    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();

    assert_eq!(client.echo_string("hello").await.unwrap(), "hello");
    assert_eq!(client.echo_string("héllo ✓").await.unwrap(), "héllo ✓");
    for data in [
        vec![0, 1, 2, 3, 0, 255, 128, 0],
        (0..=255).collect::<Vec<u8>>(),
        payload(1500),
    ] {
        assert_eq!(client.echo(&data).await.unwrap(), data);
    }

    server.stop().await;
}

#[tokio::test]
async fn echoes_empty_datagram() {
    let server = start_udp(UdpConfig::default()).await;
    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();
    assert!(client.echo(b"").await.unwrap().is_empty());
    server.stop().await;
}

#[tokio::test]
async fn echoes_eight_kib_datagram() {
    let server = start_udp(UdpConfig::default()).await;
    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();
    let data = payload(8 * 1024);
    assert_eq!(client.echo(&data).await.unwrap(), data);
    server.stop().await;
}

#[tokio::test]
async fn datagram_larger_than_server_buffer_is_truncated() {
    let server = start_udp(UdpConfig {
        buffer_size: 16,
        ..Default::default()
    })
    .await;
    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();
    let data = payload(32);
    assert_eq!(client.echo(&data).await.unwrap(), &data[..16]);
    server.stop().await;
}

#[tokio::test]
async fn concurrent_clients_get_their_own_payload() {
    let server = start_udp(UdpConfig::default()).await;
    let addr = server.addr;

    let tasks: Vec<_> = (0..20)
        .map(|id| {
            tokio::spawn(async move {
                let mut client = UdpEchoClient::connect(addr).await?;
                for round in 0..5 {
                    let data = tagged_payload(id * 10 + round, 64 + id);
                    assert_eq!(client.echo(&data).await?, data, "client {id} round {round}");
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
async fn echoes_over_ipv6() {
    let server = match try_start_udp(UdpConfig {
        bind_addr: "[::1]:0".parse().unwrap(),
        ..Default::default()
    })
    .await
    {
        Ok(server) => server,
        Err(e) => {
            eprintln!("skipping: IPv6 loopback unavailable ({e})");
            return;
        }
    };
    assert!(server.addr.is_ipv6());

    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(client.echo_string("over v6").await.unwrap(), "over v6");
    server.stop().await;
}

#[tokio::test]
async fn idle_read_timeout_does_not_stop_server() {
    // The idle receive timeout is not an error: the server keeps serving.
    let server = start_udp(UdpConfig {
        read_timeout: Duration::from_millis(20),
        ..Default::default()
    })
    .await;
    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        client.echo_string("still here").await.unwrap(),
        "still here"
    );
    server.stop().await;
}

#[tokio::test]
async fn graceful_shutdown_stops_echoing() {
    let server = start_udp(UdpConfig::default()).await;
    let mut client = UdpEchoClient::connect_with_config(server.addr, short_timeout_client())
        .await
        .unwrap();
    assert_eq!(client.echo_string("before").await.unwrap(), "before");

    let addr = server.stop().await;
    assert!(client.echo_string("after").await.is_err());

    // The port is free again once the server is gone.
    let rebound = start_udp(UdpConfig {
        bind_addr: addr,
        ..Default::default()
    })
    .await;
    assert_eq!(client.echo_string("rebound").await.unwrap(), "rebound");
    rebound.stop().await;
}

#[tokio::test]
async fn shutdown_requested_before_serve_is_honored() {
    let server = UdpEchoServer::new(UdpConfig::default().into());
    server.shutdown_signal().send(()).unwrap();
    tokio::time::timeout(WAIT, server.run())
        .await
        .expect("run ignored an early shutdown")
        .unwrap();
}

#[tokio::test]
async fn zero_buffer_size_is_rejected() {
    let server = UdpEchoServer::new(
        UdpConfig {
            buffer_size: 0,
            ..Default::default()
        }
        .into(),
    );
    assert!(matches!(server.bind().await, Err(EchoError::Config(_))));

    let err = UdpEchoClient::connect_with_config(
        "127.0.0.1:9".parse().unwrap(),
        DatagramClientConfig {
            buffer_size: 0,
            ..Default::default()
        },
    )
    .await
    .err()
    .expect("zero client buffer must be rejected");
    assert!(matches!(err, EchoError::Config(_)), "{err:?}");
}

#[tokio::test]
async fn client_times_out_without_server() {
    // Nothing listens on the address of a stopped server.
    let addr = start_udp(UdpConfig::default()).await.stop().await;
    let mut client = UdpEchoClient::connect_with_config(addr, short_timeout_client())
        .await
        .unwrap();
    let err = client.echo(b"anyone?").await.unwrap_err();
    // Either the receive times out or the kernel reports the port unreachable.
    assert!(
        matches!(err, EchoError::Timeout(_) | EchoError::Udp(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn client_ignores_datagrams_from_other_peers() {
    // A hand-rolled "server" that lets a third party reply first.
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut client = UdpEchoClient::connect(server.local_addr().unwrap())
        .await
        .unwrap();
    let responder = tokio::spawn(async move {
        let mut buf = [0u8; 64];
        let (n, peer) = server.recv_from(&mut buf).await.unwrap();
        let intruder = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        intruder.send_to(b"intruder", peer).await.unwrap();
        server.send_to(&buf[..n], peer).await.unwrap();
    });
    assert_eq!(client.echo(b"genuine").await.unwrap(), b"genuine");
    responder.await.unwrap();
}
