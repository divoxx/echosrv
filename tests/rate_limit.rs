//! Rate-limit enforcement by the servers: what a rejected client observes and
//! what the servers count.
//!
//! Every limit uses `rate_per_sec: 1`, so after the burst the next slot is a
//! full second away: far longer than these tests take to run.

mod common;

use common::{
    WAIT, socket_dir, start_http, start_tcp, start_udp, start_unix_datagram, start_unix_stream,
};
use echosrv::datagram::DatagramClientConfig;
use echosrv::http::HttpConfig;
use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
use echosrv::{
    EchoClient, EchoError, HttpEchoClient, RateLimitConfig, ServerStats, TcpConfig, TcpEchoClient,
    UdpConfig, UdpEchoClient, UnixDatagramEchoClient, UnixStreamEchoClient,
};
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket, UnixStream};

/// One event, then nothing for a second.
const ONE_PER_SEC: RateLimitConfig = RateLimitConfig::new(1, 1);

/// Waits until `cond(stats)` holds; fails after [`WAIT`].
async fn wait_for_stats(stats: &ServerStats, what: &str, cond: impl Fn(&ServerStats) -> bool) {
    tokio::time::timeout(WAIT, async {
        while !cond(stats) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}; stats: {stats:?}"));
}

/// A raw HTTP response: status code, header lines and body.
struct Response {
    status: u16,
    head: String,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

/// Sends `request` on a new connection and reads the response until EOF.
/// A reset instead of EOF fails the test: the 429 must arrive intact.
async fn http_exchange(addr: SocketAddr, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("server did not close the connection")
        .expect("connection failed instead of a clean close");
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").expect("no response head");
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("no status code");
    Response {
        status,
        head: head.to_string(),
        body: body.to_string(),
    }
}

fn post(body: &str) -> Vec<u8> {
    format!(
        "POST / HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn assert_too_many_requests(response: &Response) {
    assert_eq!(response.status, 429, "{}", response.head);
    assert!(
        response
            .head
            .starts_with("HTTP/1.1 429 Too Many Requests\r\n")
    );
    // ONE_PER_SEC: the next slot is at most 1 s away, rounded up to 1.
    assert_eq!(response.header("Retry-After"), Some("1"));
    assert_eq!(response.header("Connection"), Some("close"));
    assert_eq!(
        response.header("Content-Length"),
        Some(response.body.len().to_string().as_str())
    );
    assert!(response.body.contains("Rate limit exceeded"));
}

/// Reads from a stream the server rejected; returns the error kind, or
/// `None` for a clean EOF.
async fn read_rejection<S: AsyncReadExt + Unpin>(stream: &mut S) -> Option<ErrorKind> {
    let mut buf = [0u8; 64];
    match tokio::time::timeout(WAIT, stream.read(&mut buf))
        .await
        .expect("server neither echoed nor closed the connection")
    {
        Ok(0) => None,
        Ok(n) => panic!("expected a rejection, got {n} echoed bytes"),
        Err(e) => Some(e.kind()),
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_request_over_limit_gets_429_with_retry_after() {
    let server = start_http(HttpConfig::default().with_rate_limit(ONE_PER_SEC)).await;

    let mut client = HttpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(client.echo(b"first").await.unwrap(), b"first");

    let response = http_exchange(server.addr, &post("second")).await;
    assert_too_many_requests(&response);

    // Empty bodies are counted too (they are answered without an echo write).
    let response = http_exchange(server.addr, &post("")).await;
    assert_too_many_requests(&response);

    // The library client reports the 429 with its Retry-After.
    let mut client = HttpEchoClient::connect(server.addr).await.unwrap();
    let err = client.echo(b"third").await.unwrap_err();
    assert!(err.is_rate_limited(), "{err:?}");
    assert_eq!(err.retry_after(), Some(Duration::from_secs(1)));
    assert!(
        matches!(err, EchoError::HttpStatus { status: 429, ref body, .. } if !body.is_empty()),
        "{err:?}"
    );

    assert_eq!(server.stats.rejected_requests(), 3);
    assert_eq!(server.stats.rejected_connections(), 0);
    server.stop().await;
}

#[tokio::test]
async fn http_invalid_requests_count_against_the_limit() {
    let server = start_http(HttpConfig::default().with_rate_limit(ONE_PER_SEC)).await;

    // Admitted: answered with its error status.
    let response = http_exchange(server.addr, b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").await;
    assert_eq!(response.status, 405);
    // Over the limit: 429 instead of the 405.
    let response = http_exchange(server.addr, b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").await;
    assert_too_many_requests(&response);

    assert_eq!(server.stats.rejected_requests(), 1);
    server.stop().await;
}

#[tokio::test]
async fn http_connection_over_accept_limit_gets_429() {
    let server = start_http(HttpConfig::default().with_accept_rate_limit(ONE_PER_SEC)).await;

    let mut client = HttpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(client.echo(b"first").await.unwrap(), b"first");

    // The server decides to reject at accept time, before the request
    // arrives; it must read the request and still deliver the 429.
    let response = http_exchange(server.addr, &post("second")).await;
    assert_too_many_requests(&response);

    // The library client sees the same 429.
    let mut client = HttpEchoClient::connect(server.addr).await.unwrap();
    let err = client.echo(b"third").await.unwrap_err();
    assert!(err.is_rate_limited(), "{err:?}");
    assert!(err.retry_after().is_some(), "{err:?}");

    assert_eq!(server.stats.rejected_connections(), 2);
    assert_eq!(server.stats.rejected_requests(), 0);
    server.stop().await;
}

// ---------------------------------------------------------------------------
// TCP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tcp_connection_over_accept_limit_is_reset() {
    let server = start_tcp(TcpConfig::default().with_accept_rate_limit(ONE_PER_SEC)).await;

    let mut first = TcpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(first.echo(b"ok").await.unwrap(), b"ok");

    let mut second = TcpStream::connect(server.addr).await.unwrap();
    assert_eq!(
        read_rejection(&mut second).await,
        Some(ErrorKind::ConnectionReset)
    );
    assert_eq!(server.stats.rejected_connections(), 1);

    // The admitted connection is unaffected.
    assert_eq!(first.echo(b"still ok").await.unwrap(), b"still ok");
    assert_eq!(server.stats.rejected_requests(), 0);
    drop(first);
    server.stop().await;
}

#[tokio::test]
async fn tcp_request_over_limit_resets_the_connection() {
    let server = start_tcp(TcpConfig::default().with_rate_limit(ONE_PER_SEC)).await;

    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream.write_all(b"one").await.unwrap();
    let mut buf = [0u8; 3];
    tokio::time::timeout(WAIT, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"one");

    stream.write_all(b"two").await.unwrap();
    assert_eq!(
        read_rejection(&mut stream).await,
        Some(ErrorKind::ConnectionReset)
    );
    assert_eq!(server.stats.rejected_requests(), 1);
    assert_eq!(server.stats.rejected_connections(), 0);
    server.stop().await;
}

#[tokio::test]
async fn tcp_client_sees_rejection_as_connection_reset() {
    let server = start_tcp(TcpConfig::default().with_rate_limit(ONE_PER_SEC)).await;

    let mut client = TcpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(client.echo(b"one").await.unwrap(), b"one");
    let err = client.echo(b"two").await.unwrap_err();
    assert!(matches!(err, EchoError::Tcp(_)), "{err:?}");
    assert_eq!(
        err.io_error_kind(),
        Some(ErrorKind::ConnectionReset),
        "{err:?}"
    );
    assert!(!err.is_rate_limited());
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Unix stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unix_stream_connection_over_accept_limit_is_closed() {
    let dir = socket_dir();
    let server = start_unix_stream(
        UnixStreamConfig::default()
            .with_socket_path(dir.path().join("s.sock"))
            .with_accept_rate_limit(ONE_PER_SEC),
    )
    .await;

    let mut first = UnixStreamEchoClient::connect(server.addr.clone())
        .await
        .unwrap();
    assert_eq!(first.echo(b"ok").await.unwrap(), b"ok");

    let mut second = UnixStream::connect(&server.addr).await.unwrap();
    assert_eq!(read_rejection(&mut second).await, None, "expected EOF");
    assert_eq!(server.stats.rejected_connections(), 1);

    assert_eq!(first.echo(b"still ok").await.unwrap(), b"still ok");
    drop(first);
    server.stop().await;
}

#[tokio::test]
async fn unix_stream_request_over_limit_closes_the_connection() {
    let dir = socket_dir();
    let server = start_unix_stream(
        UnixStreamConfig::default()
            .with_socket_path(dir.path().join("s.sock"))
            .with_rate_limit(ONE_PER_SEC),
    )
    .await;

    let mut stream = UnixStream::connect(&server.addr).await.unwrap();
    stream.write_all(b"one").await.unwrap();
    let mut buf = [0u8; 3];
    tokio::time::timeout(WAIT, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"one");

    stream.write_all(b"two").await.unwrap();
    assert_eq!(read_rejection(&mut stream).await, None, "expected EOF");
    assert_eq!(server.stats.rejected_requests(), 1);
    server.stop().await;
}

#[tokio::test]
async fn unix_stream_client_sees_rejection_as_unexpected_eof() {
    let dir = socket_dir();
    let server = start_unix_stream(
        UnixStreamConfig::default()
            .with_socket_path(dir.path().join("s.sock"))
            .with_rate_limit(ONE_PER_SEC),
    )
    .await;

    let mut client = UnixStreamEchoClient::connect(server.addr.clone())
        .await
        .unwrap();
    assert_eq!(client.echo(b"one").await.unwrap(), b"one");
    // The close arrives before any echo: an error, not an empty "echo".
    let err = client.echo(b"two").await.unwrap_err();
    assert!(matches!(err, EchoError::Unix(_)), "{err:?}");
    assert_eq!(
        err.io_error_kind(),
        Some(ErrorKind::UnexpectedEof),
        "{err:?}"
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Datagrams
// ---------------------------------------------------------------------------

#[tokio::test]
async fn udp_datagrams_over_limit_are_dropped_and_counted() {
    let server = start_udp(UdpConfig::default().with_rate_limit(ONE_PER_SEC)).await;

    let mut client = UdpEchoClient::connect(server.addr).await.unwrap();
    assert_eq!(client.echo(b"first").await.unwrap(), b"first");

    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for i in 0..5u8 {
        socket.send_to(&[i], server.addr).await.unwrap();
    }
    wait_for_stats(&server.stats, "5 dropped datagrams", |s| {
        s.dropped_rate_limited() == 5
    })
    .await;

    // None of them was echoed.
    let mut buf = [0u8; 16];
    assert!(
        tokio::time::timeout(Duration::from_millis(50), socket.recv_from(&mut buf))
            .await
            .is_err(),
        "a rate-limited datagram was echoed"
    );
    assert_eq!(server.stats.rejected_requests(), 0);

    // The library client sees a dropped datagram as a receive timeout.
    let config = DatagramClientConfig {
        read_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let mut client = UdpEchoClient::connect_with_config(server.addr, config)
        .await
        .unwrap();
    assert!(matches!(
        client.echo(b"dropped").await,
        Err(EchoError::Timeout(_))
    ));
    server.stop().await;
}

#[tokio::test]
async fn unix_datagrams_over_limit_are_dropped_and_counted() {
    let dir = socket_dir();
    let server = start_unix_datagram(
        UnixDatagramConfig::default()
            .with_socket_path(dir.path().join("d.sock"))
            .with_rate_limit(RateLimitConfig::new(1, 2)),
    )
    .await;

    let mut client = UnixDatagramEchoClient::connect(server.addr.clone())
        .await
        .unwrap();
    // The burst of 2 is echoed, the rest is dropped.
    assert_eq!(client.echo(b"a").await.unwrap(), b"a");
    assert_eq!(client.echo(b"b").await.unwrap(), b"b");

    let sender = tokio::net::UnixDatagram::unbound().unwrap();
    for _ in 0..3 {
        sender.send_to(b"x", &server.addr).await.unwrap();
    }
    wait_for_stats(&server.stats, "3 dropped datagrams", |s| {
        s.dropped_rate_limited() == 3
    })
    .await;
    server.stop().await;
}
