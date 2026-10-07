//! Property-based tests: whatever is sent must come back unchanged.
//!
//! Servers are started once per test binary on a shared runtime and reused by
//! every generated case, so each case costs one loopback round trip rather
//! than a server start-up.

use echosrv::{
    EchoClient, TcpConfig, TcpEchoClient, TcpEchoServer, UdpConfig, UdpEchoClient, UdpEchoServer,
};
use proptest::prelude::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{LazyLock, Mutex};
use tokio::runtime::Runtime;

/// Shared multi-threaded runtime; servers run on its workers while test
/// threads drive their client futures with `block_on`.
static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build test runtime")
});

/// Server buffer sizes exercised by `echo_independent_of_server_buffer_size`.
const BUFFER_SIZES: [usize; 5] = [1, 7, 64, 1024, 4096];

/// Returns the address of a TCP echo server with `buffer_size`, starting it on
/// first use. Servers live until the test process exits.
fn tcp_server(buffer_size: usize) -> SocketAddr {
    static SERVERS: LazyLock<Mutex<HashMap<usize, SocketAddr>>> = LazyLock::new(Default::default);
    let mut servers = SERVERS.lock().unwrap_or_else(|p| p.into_inner());
    *servers.entry(buffer_size).or_insert_with(|| {
        RUNTIME.block_on(async {
            let server = TcpEchoServer::new(
                TcpConfig {
                    buffer_size,
                    max_connections: 1000,
                    ..Default::default()
                }
                .into(),
            );
            let bound = server.bind().await.expect("failed to bind TCP server");
            let addr = *bound.local_addr().as_network().unwrap();
            tokio::spawn(bound.serve());
            addr
        })
    })
}

fn udp_server() -> SocketAddr {
    static SERVER: LazyLock<SocketAddr> = LazyLock::new(|| {
        RUNTIME.block_on(async {
            let server = UdpEchoServer::new(UdpConfig::default().into());
            let bound = server.bind().await.expect("failed to bind UDP server");
            let addr = *bound.local_addr().as_network().unwrap();
            tokio::spawn(bound.serve());
            addr
        })
    });
    *SERVER
}

fn fail(context: &str) -> impl Fn(echosrv::EchoError) -> TestCaseError + '_ {
    move |e| TestCaseError::fail(format!("{context}: {e}"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Arbitrary bytes (including empty and NUL-heavy input) round-trip.
    #[test]
    fn echo_preserves_bytes(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let addr = tcp_server(1024);
        let response = RUNTIME.block_on(async {
            let mut client = TcpEchoClient::connect(addr).await?;
            client.echo(&data).await
        }).map_err(fail("echo"))?;
        prop_assert_eq!(response, data);
    }

    /// Arbitrary Unicode strings round-trip.
    #[test]
    fn echo_preserves_strings(text in ".*") {
        let addr = tcp_server(1024);
        let response = RUNTIME.block_on(async {
            let mut client = TcpEchoClient::connect(addr).await?;
            client.echo_string(&text).await
        }).map_err(fail("echo"))?;
        prop_assert_eq!(response, text);
    }

    /// Several messages on one connection each come back intact and in order.
    #[test]
    fn echo_preserves_message_sequence(
        messages in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..512), 1..8)
    ) {
        let addr = tcp_server(64);
        let responses = RUNTIME.block_on(async {
            let mut client = TcpEchoClient::connect(addr).await?;
            let mut responses = Vec::new();
            for message in &messages {
                responses.push(client.echo(message).await?);
            }
            Ok::<_, echosrv::EchoError>(responses)
        }).map_err(fail("echo"))?;
        prop_assert_eq!(responses, messages);
    }

    /// Concurrent clients never receive each other's data.
    #[test]
    fn concurrent_clients_get_their_own_responses(
        messages in prop::collection::vec(".+", 1..10)
    ) {
        let addr = tcp_server(1024);
        let results = RUNTIME.block_on(async {
            let tasks: Vec<_> = messages
                .iter()
                .cloned()
                .map(|message| {
                    tokio::spawn(async move {
                        let mut client = TcpEchoClient::connect(addr).await?;
                        let response = client.echo_string(&message).await?;
                        Ok::<_, echosrv::EchoError>((message, response))
                    })
                })
                .collect();
            let mut results = Vec::new();
            for task in tasks {
                results.push(task.await.expect("client task panicked")?);
            }
            Ok::<_, echosrv::EchoError>(results)
        }).map_err(fail("concurrent echo"))?;
        for (sent, received) in results {
            prop_assert_eq!(sent, received);
        }
    }

    /// The server's read buffer size does not affect what is echoed.
    #[test]
    fn echo_independent_of_server_buffer_size(
        data in prop::collection::vec(any::<u8>(), 1..8192),
        buffer_size in prop::sample::select(BUFFER_SIZES.to_vec()),
    ) {
        let addr = tcp_server(buffer_size);
        let response = RUNTIME.block_on(async {
            let mut client = TcpEchoClient::connect(addr).await?;
            client.echo(&data).await
        }).map_err(fail("echo"))?;
        prop_assert_eq!(response, data);
    }

    /// A datagram of any size up to 8 KiB round-trips unchanged.
    #[test]
    fn udp_echo_preserves_datagrams(data in prop::collection::vec(any::<u8>(), 0..8192)) {
        let addr = udp_server();
        let response = RUNTIME.block_on(async {
            let mut client = UdpEchoClient::connect(addr).await?;
            client.echo(&data).await
        }).map_err(fail("udp echo"))?;
        prop_assert_eq!(response, data);
    }
}
