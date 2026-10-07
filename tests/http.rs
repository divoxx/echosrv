//! HTTP/1.1 echo server integration tests (raw sockets + `HttpEchoClient`).

mod common;

use echosrv::http::HttpConfig;
use echosrv::{Address, EchoClient, EchoError, Result};
use std::time::Duration;

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

mod http_support {
    use echosrv::http::HttpConfig;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    pub const SERVER_NAME: &str = "IntegrationTest/1.0";
    pub const CONTENT_TYPE: &str = "application/x-echo";

    /// A test configuration with distinctive response header values.
    pub fn config() -> HttpConfig {
        HttpConfig {
            max_connections: 50,
            buffer_size: 1024,
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            server_name: Some(SERVER_NAME.to_string()),
            default_content_type: Some(CONTENT_TYPE.to_string()),
            ..HttpConfig::default()
        }
    }

    /// Starts an HTTP echo server on an ephemeral port (see `common::start_http`).
    pub async fn start(mut config: HttpConfig) -> crate::common::TestServer<SocketAddr> {
        config.bind_addr = "127.0.0.1:0".parse().unwrap();
        crate::common::start_http(config).await
    }

    #[derive(Debug)]
    pub struct Response {
        pub status: u16,
        pub reason: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Response {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }

        /// Asserts the headers every response must carry.
        pub fn assert_common_headers(&self) {
            assert_eq!(self.header("Connection"), Some("close"));
            assert_eq!(self.header("Server"), Some(SERVER_NAME));
            let len: usize = self
                .header("Content-Length")
                .expect("missing Content-Length")
                .parse()
                .unwrap();
            assert_eq!(len, self.body.len(), "Content-Length matches body");
        }
    }

    /// Parses one complete response. The body is everything after the head.
    pub fn parse(raw: &[u8]) -> Response {
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut res = httparse::Response::new(&mut headers);
        let len = match res.parse(raw).expect("invalid HTTP response") {
            httparse::Status::Complete(len) => len,
            httparse::Status::Partial => panic!(
                "incomplete HTTP response: {:?}",
                String::from_utf8_lossy(raw)
            ),
        };
        assert_eq!(res.version, Some(1), "HTTP/1.1 status line");
        Response {
            status: res.code.unwrap(),
            reason: res.reason.unwrap().to_string(),
            headers: res
                .headers
                .iter()
                .map(|h| {
                    (
                        h.name.to_string(),
                        String::from_utf8(h.value.to_vec()).unwrap(),
                    )
                })
                .collect(),
            body: raw[len..].to_vec(),
        }
    }

    /// Reads until the server closes the connection.
    pub async fn read_all(stream: &mut TcpStream) -> Vec<u8> {
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw))
            .await
            .expect("timed out waiting for the server to close")
            .expect("read failed");
        raw
    }

    /// Sends `parts` (pausing `gap` between them), then reads the response.
    pub async fn send_parts(addr: SocketAddr, parts: &[&[u8]], gap: Duration) -> Response {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 && !gap.is_zero() {
                tokio::time::sleep(gap).await;
            }
            stream.write_all(part).await.unwrap();
            stream.flush().await.unwrap();
        }
        parse(&read_all(&mut stream).await)
    }

    pub async fn send(addr: SocketAddr, request: &[u8]) -> Response {
        send_parts(addr, &[request], Duration::ZERO).await
    }

    pub fn post(body: &[u8]) -> Vec<u8> {
        let mut req = format!(
            "POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        req.extend_from_slice(body);
        req
    }

    /// Deterministic binary payload.
    pub fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }
}

#[tokio::test]
async fn test_http_response_shape() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(server.addr, &http_support::post(b"post body")).await;

    assert_eq!(res.status, 200);
    assert_eq!(res.reason, "OK");
    res.assert_common_headers();
    assert_eq!(res.header("Content-Type"), Some(http_support::CONTENT_TYPE));
    assert_eq!(res.header("Content-Length"), Some("9"));
    assert_eq!(res.body, b"post body");
    server.stop().await;
}

#[tokio::test]
async fn test_http_large_body() {
    let server = http_support::start(http_support::config()).await;
    // 64 KiB through a 1 KiB server buffer.
    let body = http_support::payload(64 * 1024);
    let res = http_support::send(server.addr, &http_support::post(&body)).await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, body);
    server.stop().await;
}

#[tokio::test]
async fn test_http_head_and_body_in_separate_writes() {
    let server = http_support::start(http_support::config()).await;
    let body = http_support::payload(5000);
    let head = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let res = http_support::send_parts(
        server.addr,
        &[
            &head.as_bytes()[..10],
            &head.as_bytes()[10..],
            &body[..1000],
            &body[1000..],
        ],
        Duration::from_millis(30),
    )
    .await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, body);
    server.stop().await;
}

#[tokio::test]
async fn test_http_byte_at_a_time() {
    let server = http_support::start(http_support::config()).await;
    let request = http_support::post(b"slow and steady");
    let parts: Vec<&[u8]> = request.chunks(1).collect();
    let res = http_support::send_parts(server.addr, &parts, Duration::ZERO).await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, b"slow and steady");
    server.stop().await;
}

#[tokio::test]
async fn test_http_empty_post() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(server.addr, &http_support::post(b"")).await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.header("Content-Length"), Some("0"));
    assert!(res.body.is_empty());
    server.stop().await;
}

#[tokio::test]
async fn test_http_missing_content_length_is_empty_body() {
    let server = http_support::start(http_support::config()).await;
    // Without Content-Length the body is empty; trailing bytes are ignored.
    let res = http_support::send(
        server.addr,
        b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\nignored",
    )
    .await;

    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert!(res.body.is_empty());
    server.stop().await;
}

#[tokio::test]
async fn test_http_chunked_is_not_implemented() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(
        server.addr,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    )
    .await;

    assert_eq!(res.status, 501);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_too_many_headers() {
    let server = http_support::start(http_support::config()).await;
    let mut request = String::from("POST / HTTP/1.1\r\nHost: localhost\r\n");
    for i in 0..32 {
        request.push_str(&format!("X-Header-{i}: value\r\n"));
    }
    request.push_str("Content-Length: 0\r\n\r\n");
    let res = http_support::send(server.addr, request.as_bytes()).await;

    assert_eq!(res.status, 400);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_header_section_too_large() {
    let server = http_support::start(http_support::config()).await;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nX-Big: {}\r\nContent-Length: 0\r\n\r\n",
        "a".repeat(9 * 1024)
    );
    let res = http_support::send(server.addr, request.as_bytes()).await;

    assert_eq!(res.status, 431);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_invalid_content_length() {
    let server = http_support::start(http_support::config()).await;
    for value in ["abc", "-5", "5, 6", "1.5"] {
        let request =
            format!("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {value}\r\n\r\n");
        let res = http_support::send(server.addr, request.as_bytes()).await;
        assert_eq!(res.status, 400, "Content-Length: {value}");
        res.assert_common_headers();
    }
    server.stop().await;
}

#[tokio::test]
async fn test_http_malformed_request_line() {
    let server = http_support::start(http_support::config()).await;
    let res = http_support::send(server.addr, b"NOT AN HTTP REQUEST\r\n\r\n").await;
    assert_eq!(res.status, 400);
    res.assert_common_headers();
    server.stop().await;
}

#[tokio::test]
async fn test_http_body_too_large() {
    let server = http_support::start(HttpConfig {
        max_body_size: 16,
        ..http_support::config()
    })
    .await;

    // Exactly at the limit is fine.
    let res = http_support::send(server.addr, &http_support::post(&[b'a'; 16])).await;
    assert_eq!(res.status, 200);
    assert_eq!(res.body, [b'a'; 16]);

    // One byte over is rejected, even though the body is sent anyway.
    let res = http_support::send(server.addr, &http_support::post(&[b'a'; 17])).await;
    assert_eq!(res.status, 413);
    res.assert_common_headers();

    // A huge declared length is rejected without waiting for the body.
    let res = http_support::send(
        server.addr,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000000\r\n\r\n",
    )
    .await;
    assert_eq!(res.status, 413);
    server.stop().await;
}

#[tokio::test]
async fn test_http_expect_100_continue() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server = http_support::start(http_support::config()).await;
    let body = http_support::payload(2048);
    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let head = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();

    // The server must invite the body before we send it.
    let expected = b"HTTP/1.1 100 Continue\r\n\r\n";
    let mut interim = [0u8; 25];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut interim))
        .await
        .expect("no 100 Continue")
        .unwrap();
    assert_eq!(&interim, expected);

    stream.write_all(&body).await.unwrap();
    let res = http_support::parse(&http_support::read_all(&mut stream).await);
    assert_eq!(res.status, 200);
    res.assert_common_headers();
    assert_eq!(res.body, body);
    server.stop().await;
}

#[tokio::test]
async fn test_http_method_not_allowed() {
    let server = http_support::start(http_support::config()).await;
    for method in ["GET", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
        let request = format!("{method} / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let res = http_support::send(server.addr, request.as_bytes()).await;

        assert_eq!(res.status, 405, "{method}");
        assert_eq!(res.reason, "Method Not Allowed");
        assert_eq!(res.header("Allow"), Some("POST"));
        res.assert_common_headers();
        let body = String::from_utf8(res.body).unwrap();
        assert!(
            body.contains(&format!("Method {method} not allowed")),
            "{body}"
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn test_http_client_round_trip() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(http_support::config()).await;
    let mut client = HttpEchoClient::connect(Address::Network(server.addr)).await?;

    assert_eq!(client.echo_string("hello").await?, "hello");
    // The server closes after each response; the client reconnects.
    assert_eq!(client.echo(b"").await?, b"");
    let body = http_support::payload(100 * 1024);
    assert_eq!(client.echo(&body).await?, body);

    server.stop().await;
    Ok(())
}

/// Bodies larger than the socket buffers must not deadlock: the server echoes
/// while it is still receiving, so the client reads while it writes.
#[tokio::test]
async fn test_http_client_echoes_eight_mebibyte_body() -> Result<()> {
    use echosrv::http::HttpEchoClient;
    use echosrv::stream::ClientConfig;

    let server = http_support::start(HttpConfig {
        buffer_size: 64 * 1024,
        max_body_size: 16 * 1024 * 1024,
        read_timeout: common::WAIT,
        write_timeout: common::WAIT,
        ..http_support::config()
    })
    .await;
    let config = ClientConfig {
        read_timeout: common::WAIT,
        write_timeout: common::WAIT,
        ..Default::default()
    };
    let mut client = HttpEchoClient::connect_with_config(server.addr, config).await?;
    let body = http_support::payload(8 * 1024 * 1024);
    let echoed = client.echo(&body).await?;
    assert!(echoed == body, "8 MiB body was not echoed intact");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn test_http_client_reports_error_status() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(HttpConfig {
        max_body_size: 4,
        ..http_support::config()
    })
    .await;
    let mut client = HttpEchoClient::connect(server.addr).await?;
    let err = client
        .echo(b"too long")
        .await
        .expect_err("413 must be an error");
    assert!(err.to_string().contains("413"), "{err}");

    server.stop().await;
    Ok(())
}

/// A large body rejected up front (413) is reported as the HTTP error, not as
/// the write failure caused by the server no longer reading.
#[tokio::test]
async fn test_http_client_reports_413_for_large_body() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(HttpConfig {
        max_body_size: 4,
        ..http_support::config()
    })
    .await;
    let mut client = HttpEchoClient::connect(server.addr).await?;
    let err = client
        .echo(&http_support::payload(8 * 1024 * 1024))
        .await
        .expect_err("413 must be an error");
    assert!(err.to_string().contains("413"), "{err}");

    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn test_http_concurrent_clients() -> Result<()> {
    use echosrv::http::HttpEchoClient;

    let server = http_support::start(http_support::config()).await;
    let addr = server.addr;
    let handles: Vec<_> = (0..10)
        .map(|i| {
            tokio::spawn(async move {
                let mut client = HttpEchoClient::connect(addr).await?;
                let message = format!("concurrent client {i}");
                let response = client.echo_string(&message).await?;
                assert_eq!(response, message);
                Ok::<(), EchoError>(())
            })
        })
        .collect();

    for handle in handles {
        handle
            .await
            .map_err(|e| EchoError::Config(format!("Task join error: {e}")))??;
    }
    server.stop().await;
    Ok(())
}
