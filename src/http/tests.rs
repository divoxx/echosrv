use super::client::HttpEchoClient;
use super::config::HttpConfig;
use super::protocol::{
    HTTP_SETTINGS, HeadOutcome, HttpProtocol, HttpProtocolError, HttpSettings, MAX_HEADER_BYTES,
    Rejection, RequestHead, parse_content_length, parse_head, response_head,
};
use crate::common::EchoClient;
use crate::stream::{RejectReason, StreamProtocol};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const MAX_BODY: usize = 1024;

fn status_of(outcome: &HeadOutcome) -> Option<u16> {
    match outcome {
        HeadOutcome::Reject(r) => Some(r.status),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pure parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_head_complete_post() {
    let req = b"POST /x HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nhello";
    assert_eq!(
        parse_head(req, MAX_BODY),
        HeadOutcome::Request(RequestHead {
            head_len: req.len() - 5,
            content_length: 5,
            expect_continue: false,
        })
    );
}

#[test]
fn parse_head_partial_at_every_prefix() {
    let req = b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\n";
    for end in 1..req.len() {
        assert_eq!(
            parse_head(&req[..end], MAX_BODY),
            HeadOutcome::Partial,
            "prefix of length {end}"
        );
    }
    assert!(matches!(parse_head(req, MAX_BODY), HeadOutcome::Request(_)));
}

#[test]
fn parse_head_missing_content_length_is_empty_body() {
    let outcome = parse_head(b"POST / HTTP/1.1\r\nHost: a\r\n\r\n", MAX_BODY);
    assert!(matches!(
        outcome,
        HeadOutcome::Request(RequestHead {
            content_length: 0,
            ..
        })
    ));
}

#[test]
fn parse_head_rejects_non_post_with_allow() {
    for method in ["GET", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
        let req = format!("{method} / HTTP/1.1\r\nHost: a\r\n\r\n");
        let HeadOutcome::Reject(r) = parse_head(req.as_bytes(), MAX_BODY) else {
            panic!("{method} should be rejected");
        };
        assert_eq!(r.status, 405);
        assert!(r.allow_post);
        assert!(r.message.contains(&format!("Method {method} not allowed")));
    }
}

#[test]
fn parse_head_rejects_transfer_encoding_with_501() {
    for te in ["chunked", "gzip, chunked", "identity"] {
        let req = format!("POST / HTTP/1.1\r\nTransfer-Encoding: {te}\r\n\r\n");
        assert_eq!(status_of(&parse_head(req.as_bytes(), MAX_BODY)), Some(501));
    }
}

#[test]
fn parse_head_content_length_validation() {
    let cases: &[(&str, Option<u16>)] = &[
        ("Content-Length: abc\r\n", Some(400)),
        ("Content-Length: -1\r\n", Some(400)),
        ("Content-Length: +5\r\n", Some(400)),
        ("Content-Length: 5, 5\r\n", Some(400)),
        ("Content-Length: \r\n", Some(400)),
        ("Content-Length: 99999999999999999999999999\r\n", Some(400)),
        ("Content-Length: 5\r\nContent-Length: 6\r\n", Some(400)),
        ("Content-Length: 5\r\nContent-Length: 5\r\n", None),
        ("content-length:  7 \r\n", None),
        ("Content-Length: 1024\r\n", None),
        ("Content-Length: 1025\r\n", Some(413)),
    ];
    for (header, expected) in cases {
        let req = format!("POST / HTTP/1.1\r\n{header}\r\n");
        assert_eq!(
            status_of(&parse_head(req.as_bytes(), MAX_BODY)),
            *expected,
            "header {header:?}"
        );
    }
}

#[test]
fn parse_head_too_many_headers_is_400() {
    let mut req = String::from("POST / HTTP/1.1\r\n");
    for i in 0..33 {
        req.push_str(&format!("X-H{i}: v\r\n"));
    }
    req.push_str("\r\n");
    assert_eq!(status_of(&parse_head(req.as_bytes(), MAX_BODY)), Some(400));
}

#[test]
fn parse_head_garbage_is_400() {
    assert_eq!(
        status_of(&parse_head(b"\x01\x02 nonsense\r\n\r\n", MAX_BODY)),
        Some(400)
    );
}

#[test]
fn parse_head_oversized_head_is_431() {
    let mut req = b"POST / HTTP/1.1\r\nX-Big: ".to_vec();
    req.resize(MAX_HEADER_BYTES, b'a');
    assert_eq!(status_of(&parse_head(&req, MAX_BODY)), Some(431));
    // One byte less is still just incomplete.
    assert_eq!(
        parse_head(&req[..MAX_HEADER_BYTES - 1], MAX_BODY),
        HeadOutcome::Partial
    );
}

#[test]
fn parse_head_expect_continue_only_on_http11() {
    let expect = |req: &[u8]| match parse_head(req, MAX_BODY) {
        HeadOutcome::Request(h) => h.expect_continue,
        other => panic!("unexpected {other:?}"),
    };
    assert!(expect(
        b"POST / HTTP/1.1\r\nExpect: 100-Continue\r\nContent-Length: 3\r\n\r\n"
    ));
    assert!(!expect(
        b"POST / HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 3\r\n\r\n"
    ));
    assert!(!expect(b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\n"));
}

#[test]
fn parse_content_length_values() {
    assert_eq!(parse_content_length(b"0"), Some(0));
    assert_eq!(parse_content_length(b" 42\t"), Some(42));
    assert_eq!(parse_content_length(b"4 2"), None);
    assert_eq!(parse_content_length(b"0x10"), None);
}

#[test]
fn response_head_format() {
    let head = response_head(200, "OK", Some("S/1"), Some("text/plain"), 5, &[]);
    assert_eq!(
        head,
        b"HTTP/1.1 200 OK\r\nServer: S/1\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\n"
    );
    let head = response_head(
        405,
        "Method Not Allowed",
        None,
        None,
        0,
        &[("Allow", "POST".to_string())],
    );
    assert_eq!(
        head,
        b"HTTP/1.1 405 Method Not Allowed\r\nAllow: POST\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn too_many_requests_rounds_retry_after_up_to_whole_seconds() {
    for (retry_after, secs) in [
        (Duration::ZERO, 1),
        (Duration::from_nanos(1), 1),
        (Duration::from_millis(999), 1),
        (Duration::from_secs(1), 1),
        (Duration::from_millis(1001), 2),
        (Duration::from_millis(2500), 3),
        (Duration::from_secs(60), 60),
    ] {
        let rejection = Rejection::too_many_requests(retry_after);
        assert_eq!(rejection.status, 429);
        assert_eq!(rejection.reason, "Too Many Requests");
        assert_eq!(rejection.retry_after_secs, Some(secs), "{retry_after:?}");
        assert!(!rejection.allow_post);
    }
}

#[test]
fn settings_carry_http_config_fields() {
    let config = HttpConfig {
        server_name: Some("X".into()),
        default_content_type: None,
        max_body_size: 7,
        ..HttpConfig::default()
    };
    assert_eq!(
        HttpSettings::from(&config),
        HttpSettings {
            server_name: Some("X".into()),
            content_type: None,
            max_body_size: 7,
        }
    );
}

// ---------------------------------------------------------------------------
// Stream state machine over real sockets
// ---------------------------------------------------------------------------

/// Serves one connection with the same read/echo loop as `StreamEchoServer`.
async fn serve_one(
    settings: HttpSettings,
    buffer_size: usize,
) -> (SocketAddr, JoinHandle<Result<(), HttpProtocolError>>) {
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(HTTP_SETTINGS.scope(Arc::new(settings), async move {
        let (mut stream, _) = HttpProtocol::accept(&mut listener).await?;
        let mut buffer = vec![0u8; buffer_size];
        loop {
            let n = HttpProtocol::read(&mut stream, &mut buffer).await?;
            if n == 0 {
                return Ok(());
            }
            HttpProtocol::write(&mut stream, &buffer[..n]).await?;
            HttpProtocol::flush(&mut stream).await?;
        }
    }));
    (addr, handle)
}

fn test_settings() -> HttpSettings {
    HttpSettings {
        server_name: Some("UnitTest/1".into()),
        content_type: Some("application/octet-stream".into()),
        max_body_size: MAX_BODY,
    }
}

/// Sends `parts` with a pause between them and returns the raw response.
async fn exchange(addr: SocketAddr, parts: &[&[u8]]) -> Vec<u8> {
    let mut client = TcpStream::connect(addr).await.unwrap();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        client.write_all(part).await.unwrap();
    }
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut response))
        .await
        .expect("response timed out")
        .unwrap();
    response
}

fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
    let pos = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no end of head");
    (
        String::from_utf8(raw[..pos + 4].to_vec()).unwrap(),
        raw[pos + 4..].to_vec(),
    )
}

#[tokio::test]
async fn serves_buffered_body_before_reading_socket() {
    // A 3-byte buffer forces many reads across buffered and socket bytes.
    let (addr, server) = serve_one(test_settings(), 3).await;
    let raw = exchange(
        addr,
        &[
            b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\n0123456",
            b"789",
        ],
    )
    .await;
    let (head, body) = split_response(&raw);
    assert_eq!(
        head,
        "HTTP/1.1 200 OK\r\nServer: UnitTest/1\r\nContent-Type: application/octet-stream\r\nContent-Length: 10\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(body, b"0123456789");
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn pipelined_bytes_after_body_are_ignored() {
    let (addr, server) = serve_one(test_settings(), 1024).await;
    let raw = exchange(
        addr,
        &[b"POST / HTTP/1.1\r\nContent-Length: 2\r\n\r\nhiPOST / HTTP/1.1\r\nContent-Length: 3\r\n\r\nbye"],
    )
    .await;
    let (head, body) = split_response(&raw);
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("Content-Length: 2\r\n"));
    assert_eq!(body, b"hi");
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn empty_body_gets_response_without_write() {
    let settings = HttpSettings {
        server_name: None,
        content_type: None,
        max_body_size: MAX_BODY,
    };
    let (addr, server) = serve_one(settings, 1024).await;
    let raw = exchange(addr, &[b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n"]).await;
    assert_eq!(
        raw,
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn rejection_is_a_clean_close_not_an_error() {
    let (addr, server) = serve_one(test_settings(), 1024).await;
    let raw = exchange(addr, &[b"GET / HTTP/1.1\r\nHost: a\r\n\r\n"]).await;
    let (head, body) = split_response(&raw);
    assert!(head.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
    assert!(head.contains("Allow: POST\r\n"));
    assert!(head.contains("Server: UnitTest/1\r\n"));
    assert_eq!(
        body,
        b"Method GET not allowed. Only POST requests are accepted."
    );
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn early_close_mid_head_is_quiet() {
    let (addr, server) = serve_one(test_settings(), 1024).await;
    let mut client = TcpStream::connect(addr).await.unwrap();
    client.write_all(b"POST / HTTP/1.1\r\nHo").await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(response.is_empty());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn early_close_mid_body_is_quiet() {
    let (addr, server) = serve_one(test_settings(), 1024).await;
    let mut client = TcpStream::connect(addr).await.unwrap();
    client
        .write_all(b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc")
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    // The 200 head was committed with the declared length; the truncated
    // body plus the close tells the client the exchange failed.
    let (head, body) = split_response(&response);
    assert!(head.contains("Content-Length: 10\r\n"));
    assert_eq!(body, b"abc");
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn write_misuse_is_rejected() {
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(addr).await.unwrap();
    let (mut stream, _) = HttpProtocol::accept(&mut listener).await.unwrap();

    // Writing before a request body is available.
    assert!(matches!(
        HttpProtocol::write(&mut stream, b"x").await,
        Err(HttpProtocolError::InvalidRequest(_))
    ));

    client
        .write_all(b"POST / HTTP/1.1\r\nContent-Length: 2\r\n\r\nab")
        .await
        .unwrap();
    let mut buf = [0u8; 16];
    let n = HttpProtocol::read(&mut stream, &mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"ab");
    // Echoing more than Content-Length.
    assert!(matches!(
        HttpProtocol::write(&mut stream, b"abc").await,
        Err(HttpProtocolError::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn accept_outside_http_server_uses_default_settings() {
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = HttpProtocol::accept(&mut listener).await.unwrap();
        let mut buf = [0u8; 64];
        while HttpProtocol::read(&mut stream, &mut buf).await.unwrap() != 0 {}
    });
    let raw = exchange(addr, &[b"POST / HTTP/1.1\r\n\r\n"]).await;
    let (head, _) = split_response(&raw);
    let defaults = HttpConfig::default();
    assert!(head.contains(&format!("Server: {}\r\n", defaults.server_name.unwrap())));
    assert!(head.contains(&format!(
        "Content-Type: {}\r\n",
        defaults.default_content_type.unwrap()
    )));
    server.await.unwrap();
}

// ---------------------------------------------------------------------------
// Rate-limit rejection
// ---------------------------------------------------------------------------

/// Accepts one connection, optionally runs `begin_request`, then rejects it
/// with `reason`. Returns what `begin_request` returned (or `None`).
async fn reject_one(
    reason: RejectReason,
    retry_after: Duration,
) -> (SocketAddr, JoinHandle<Option<bool>>) {
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(HTTP_SETTINGS.scope(Arc::new(test_settings()), async move {
        let (mut stream, _) = HttpProtocol::accept(&mut listener).await.unwrap();
        let begun = match reason {
            RejectReason::RateLimited => {
                Some(HttpProtocol::begin_request(&mut stream).await.unwrap())
            }
            RejectReason::ConnectionRateLimited | RejectReason::TooManyConnections => None,
        };
        HttpProtocol::reject(&mut stream, reason, retry_after)
            .await
            .unwrap();
        begun
    }));
    (addr, handle)
}

fn assert_429(raw: &[u8], retry_after_secs: u64) {
    let (head, body) = split_response(raw);
    assert!(
        head.starts_with("HTTP/1.1 429 Too Many Requests\r\n"),
        "{head}"
    );
    assert!(
        head.contains(&format!("Retry-After: {retry_after_secs}\r\n")),
        "{head}"
    );
    assert!(head.contains("Connection: close\r\n"), "{head}");
    assert!(head.contains("Server: UnitTest/1\r\n"), "{head}");
    assert!(
        head.contains(&format!("Content-Length: {}\r\n", body.len())),
        "{head}"
    );
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("Rate limit exceeded")
    );
}

#[tokio::test]
async fn rate_limited_request_gets_429_instead_of_echo() {
    let (addr, server) = reject_one(RejectReason::RateLimited, Duration::from_millis(1500)).await;
    let raw = exchange(
        addr,
        &[b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"],
    )
    .await;
    assert_429(&raw, 2);
    assert_eq!(server.await.unwrap(), Some(true));
}

#[tokio::test]
async fn connection_rejected_before_request_reads_head_then_429() {
    let (addr, server) = reject_one(
        RejectReason::ConnectionRateLimited,
        Duration::from_millis(10),
    )
    .await;
    // The request arrives in pieces after the server already decided to
    // reject; the 429 must still be delivered intact (no RST).
    let raw = exchange(
        addr,
        &[
            b"POST / HTTP/1.1\r\nHost: a\r\n",
            b"Content-Length: 4\r\n\r\n",
            b"body",
        ],
    )
    .await;
    assert_429(&raw, 1);
    assert_eq!(server.await.unwrap(), None);
}

#[tokio::test]
async fn connection_over_capacity_reads_head_then_503() {
    let (addr, server) = reject_one(RejectReason::TooManyConnections, Duration::ZERO).await;
    let raw = exchange(
        addr,
        &[
            b"POST / HTTP/1.1\r\nHost: a\r\n",
            b"Content-Length: 4\r\n\r\nbody",
        ],
    )
    .await;
    let (head, body) = split_response(&raw);
    assert!(
        head.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "{head}"
    );
    assert!(head.contains("Connection: close\r\n"), "{head}");
    assert!(!head.contains("Retry-After"), "{head}");
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("Too many connections")
    );
    assert_eq!(server.await.unwrap(), None);
}

#[tokio::test]
async fn invalid_request_counts_as_a_request_and_can_be_rate_limited() {
    // begin_request admits an invalid head as a request; if it is then rate
    // limited, the 429 replaces the 405.
    let (addr, server) = reject_one(RejectReason::RateLimited, Duration::from_millis(1)).await;
    let raw = exchange(addr, &[b"GET / HTTP/1.1\r\nHost: a\r\n\r\n"]).await;
    assert_429(&raw, 1);
    assert_eq!(raw.windows(9).filter(|w| w == b"HTTP/1.1 ").count(), 1);
    assert_eq!(server.await.unwrap(), Some(true));
}

#[tokio::test]
async fn invalid_request_gets_its_error_response_after_begin_request() {
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = HttpProtocol::accept(&mut listener).await.unwrap();
        assert!(HttpProtocol::begin_request(&mut stream).await.unwrap());
        let mut buf = [0u8; 16];
        assert_eq!(HttpProtocol::read(&mut stream, &mut buf).await.unwrap(), 0);
        // Rejecting a connection that was already answered sends nothing more.
        HttpProtocol::reject(&mut stream, RejectReason::RateLimited, Duration::ZERO)
            .await
            .unwrap();
    });
    let raw = exchange(addr, &[b"GET / HTTP/1.1\r\n\r\n"]).await;
    let (head, _) = split_response(&raw);
    assert!(head.starts_with("HTTP/1.1 405 "), "{head}");
    assert_eq!(raw.windows(9).filter(|w| w == b"HTTP/1.1 ").count(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn begin_request_is_false_when_peer_closes() {
    let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).await.unwrap();
    let (mut stream, _) = HttpProtocol::accept(&mut listener).await.unwrap();
    drop(client);
    assert!(!HttpProtocol::begin_request(&mut stream).await.unwrap());
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A fake server that ignores the request and replies with `response`.
async fn canned_server(response: &'static [u8]) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let _ = stream.read(&mut buf).await.unwrap();
        stream.write_all(response).await.unwrap();
        stream.shutdown().await.unwrap();
    });
    (addr, handle)
}

#[tokio::test]
async fn client_skips_interim_responses_and_reads_to_eof_without_length() {
    let (addr, server) = canned_server(
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabc",
    )
    .await;
    let mut client = HttpEchoClient::connect(addr).await.unwrap();
    assert_eq!(client.echo(b"abc").await.unwrap(), b"abc");
    server.await.unwrap();
}

#[tokio::test]
async fn client_errors_on_non_2xx() {
    let (addr, server) =
        canned_server(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope").await;
    let mut client = HttpEchoClient::connect(addr).await.unwrap();
    let err = client.echo(b"x").await.unwrap_err();
    match &err {
        crate::EchoError::HttpStatus {
            status,
            reason,
            retry_after,
            body,
        } => {
            assert_eq!(*status, 404);
            assert_eq!(reason, "Not Found");
            assert_eq!(*retry_after, None);
            assert_eq!(body, "nope");
        }
        other => panic!("expected HttpStatus, got {other:?}"),
    }
    assert!(!err.is_rate_limited());
    assert_eq!(err.to_string(), "HTTP error: HTTP 404 Not Found: nope");
    server.await.unwrap();
}

#[tokio::test]
async fn client_reports_429_with_retry_after() {
    let (addr, server) = canned_server(
        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 4\r\n\r\nslow",
    )
    .await;
    let mut client = HttpEchoClient::connect(addr).await.unwrap();
    let err = client.echo(b"x").await.unwrap_err();
    assert!(err.is_rate_limited(), "{err:?}");
    assert_eq!(err.retry_after(), Some(std::time::Duration::from_secs(7)));
    assert!(matches!(err, crate::EchoError::HttpStatus { ref body, .. } if body == "slow"));
    server.await.unwrap();
}

#[tokio::test]
async fn client_errors_on_close_before_response_head() {
    for response in [&b""[..], b"HTTP/1.1 200 OK\r\nContent-Le"] {
        let (addr, server) = canned_server(response).await;
        let mut client = HttpEchoClient::connect(addr).await.unwrap();
        let err = client.echo(b"x").await.unwrap_err();
        assert!(matches!(err, crate::EchoError::Tcp(_)), "{err:?}");
        assert_eq!(
            err.io_error_kind(),
            Some(std::io::ErrorKind::UnexpectedEof),
            "{err:?}"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn client_reads_in_buffer_size_chunks() {
    let server = crate::HttpEchoServer::new(crate::HttpConfig::default());
    let bound = server.bind().await.unwrap();
    let addr = *bound.local_addr().as_network().unwrap();
    let serving = tokio::spawn(bound.serve());

    let config = crate::stream::ClientConfigBuilder::new()
        .buffer_size(3)
        .build();
    let mut client = HttpEchoClient::connect_with_config(addr, config)
        .await
        .unwrap();
    assert_eq!(client.config().buffer_size, 3);
    let payload: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
    assert_eq!(client.echo(&payload).await.unwrap(), payload);
    serving.abort();
}

#[tokio::test]
async fn client_errors_on_truncated_body() {
    let (addr, server) = canned_server(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort").await;
    let mut client = HttpEchoClient::connect(addr).await.unwrap();
    let err = client.echo(b"x").await.unwrap_err();
    assert!(matches!(err, crate::EchoError::Tcp(_)), "{err:?}");
    assert_eq!(err.io_error_kind(), Some(std::io::ErrorKind::UnexpectedEof));
    let err = err.to_string();
    assert!(err.contains("truncated"), "{err}");
    server.await.unwrap();
}

#[tokio::test]
async fn client_rejects_unix_addresses() {
    let err = HttpEchoClient::connect(std::path::PathBuf::from("/tmp/nope.sock"))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::EchoError::Unsupported(_)));
}
