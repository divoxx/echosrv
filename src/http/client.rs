//! A minimal HTTP/1.1 client for the HTTP echo server.

use super::protocol::MAX_HEADERS;
use crate::common::EchoClient;
use crate::network::Address;
use crate::stream::ClientConfig;
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Largest response head the client accepts.
const MAX_RESPONSE_HEAD_BYTES: usize = 64 * 1024;

/// HTTP/1.1 echo client.
///
/// Each [`echo`](EchoClient::echo) call sends `POST / HTTP/1.1` with `Host`,
/// `Content-Length` and `Connection: close` headers. It then parses the
/// response and returns its body. The server closes the connection after
/// every response, so each call after the first opens a new connection.
///
/// Interim `1xx` responses are skipped. Errors:
///
/// * A non-2xx status is [`EchoError::HttpStatus`], with the status code,
///   reason, body and any `Retry-After` delay.
///   [`EchoError::is_rate_limited`] is true for `429 Too Many Requests`.
/// * A connection closed before the response head or the
///   `Content-Length` body is complete is an
///   [`UnexpectedEof`](std::io::ErrorKind::UnexpectedEof) [`EchoError::Tcp`]
///   error, so a partial body is never returned as success.
/// * Connect, read and write timeouts are [`EchoError::Timeout`]; connection
///   failures are [`EchoError::Tcp`] with the original
///   [`io::ErrorKind`] (e.g. `ConnectionRefused`).
///
/// Responses are read in chunks of [`ClientConfig::buffer_size`] bytes.
///
/// # Examples
///
/// ```
/// use echosrv::http::HttpEchoClient;
/// use echosrv::EchoClient;
/// # use echosrv::http::{HttpConfig, HttpEchoServer};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// # let server = HttpEchoServer::new(HttpConfig::default());
/// # let bound = server.bind().await?;
/// # let addr = *bound.local_addr().as_network().unwrap();
/// # tokio::spawn(bound.serve());
/// let mut client = HttpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo_string("hello").await?, "hello");
/// assert_eq!(client.echo_string("again").await?, "again"); // new connection
/// # Ok(())
/// # }
/// ```
///
/// Handling a rate-limited server:
///
/// ```
/// use echosrv::http::{HttpConfig, HttpEchoClient, HttpEchoServer};
/// use echosrv::{EchoClient, RateLimitConfig};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> echosrv::Result<()> {
/// // One request per second, no extra burst.
/// let config = HttpConfig::default().with_rate_limit(RateLimitConfig::new(1, 1));
/// let bound = HttpEchoServer::new(config).bind().await?;
/// let addr = *bound.local_addr().as_network().unwrap();
/// tokio::spawn(bound.serve());
///
/// let mut client = HttpEchoClient::connect(addr).await?;
/// assert_eq!(client.echo(b"first").await?, b"first");
/// match client.echo(b"second").await {
///     Err(err) if err.is_rate_limited() => {
///         // The server says when to come back (whole seconds, at least 1).
///         assert!(err.retry_after().is_some());
///     }
///     other => panic!("expected 429, got {other:?}"),
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct HttpEchoClient {
    addr: SocketAddr,
    config: ClientConfig,
    /// Connection opened by `connect`, used by the first request.
    pending: Option<TcpStream>,
}

/// A parsed response head.
struct ResponseHead {
    head_len: usize,
    status: u16,
    reason: String,
    content_length: Option<usize>,
    retry_after: Option<Duration>,
}

/// An [`UnexpectedEof`](io::ErrorKind::UnexpectedEof) error: the server
/// closed the connection before the response was complete.
fn unexpected_eof(message: String) -> EchoError {
    EchoError::Tcp(io::Error::new(io::ErrorKind::UnexpectedEof, message))
}

impl HttpEchoClient {
    /// Connects to the HTTP echo server at `address` with the default
    /// [`ClientConfig`].
    ///
    /// Returns [`EchoError::Unsupported`] for Unix socket addresses.
    pub async fn connect<A: Into<Address>>(address: A) -> Result<Self> {
        Self::connect_with_config(address, ClientConfig::default()).await
    }

    /// Connects to the HTTP echo server at `address`. `config` sets the
    /// connect, read and write timeouts, the read chunk size
    /// (`buffer_size`) and the maximum response body size.
    ///
    /// Returns [`EchoError::Unsupported`] for Unix socket addresses.
    pub async fn connect_with_config<A: Into<Address>>(
        address: A,
        config: ClientConfig,
    ) -> Result<Self> {
        let addr = match address.into() {
            Address::Network(addr) => addr,
            Address::Unix(path) => {
                return Err(EchoError::Unsupported(format!(
                    "HTTP echo client does not support Unix socket {}",
                    path.display()
                )));
            }
        };
        let stream = Self::open(addr, &config).await?;
        Ok(Self {
            addr,
            config,
            pending: Some(stream),
        })
    }

    /// Returns the client configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    async fn open(addr: SocketAddr, config: &ClientConfig) -> Result<TcpStream> {
        timeout(config.connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| EchoError::Timeout("Connection timeout".to_string()))?
            .map_err(EchoError::Tcp)
    }

    /// Reads one chunk (at most `chunk.len()` bytes) and appends it to `buf`.
    async fn read_some<R: AsyncRead + Unpin>(
        &self,
        stream: &mut R,
        chunk: &mut [u8],
        buf: &mut Vec<u8>,
    ) -> Result<usize> {
        let n = timeout(self.config.read_timeout, stream.read(chunk))
            .await
            .map_err(|_| EchoError::Timeout("Read timeout".to_string()))?
            .map_err(EchoError::Tcp)?;
        buf.extend_from_slice(&chunk[..n]);
        Ok(n)
    }

    /// Reads the final (non-1xx) response head. Body bytes received after it
    /// are left in `buf`.
    async fn read_head<R: AsyncRead + Unpin>(
        &self,
        stream: &mut R,
        chunk: &mut [u8],
        buf: &mut Vec<u8>,
    ) -> Result<ResponseHead> {
        loop {
            if let Some(head) = parse_response_head(buf)? {
                if (100..200).contains(&head.status) {
                    buf.drain(..head.head_len);
                    continue;
                }
                return Ok(head);
            }
            if buf.len() > MAX_RESPONSE_HEAD_BYTES {
                return Err(EchoError::Http(format!(
                    "HTTP response head exceeds {MAX_RESPONSE_HEAD_BYTES} bytes"
                )));
            }
            if self.read_some(stream, chunk, buf).await? == 0 {
                return Err(unexpected_eof(
                    "Connection closed before the HTTP response head was complete".to_string(),
                ));
            }
        }
    }

    /// Reads the response and returns its body. Errors on a non-2xx status,
    /// a body larger than `max_response_size`, or a truncated body.
    async fn read_response<R: AsyncRead + Unpin>(&self, stream: &mut R) -> Result<Vec<u8>> {
        let mut chunk = vec![0u8; self.config.buffer_size.max(1)];
        let mut buf = Vec::new();
        let head = self.read_head(stream, &mut chunk, &mut buf).await?;
        buf.drain(..head.head_len);

        let max = self.config.max_response_size;
        if head.content_length.is_some_and(|len| len > max) {
            return Err(EchoError::Config(format!(
                "Response size exceeds maximum of {max} bytes"
            )));
        }
        // Without Content-Length, the body is delimited by connection close.
        let expected = head.content_length.unwrap_or(usize::MAX);
        while buf.len() < expected {
            if buf.len() > max {
                return Err(EchoError::Config(format!(
                    "Response size exceeds maximum of {max} bytes"
                )));
            }
            if self.read_some(stream, &mut chunk, &mut buf).await? == 0 {
                if head.content_length.is_some() {
                    return Err(unexpected_eof(format!(
                        "HTTP response body truncated: expected {expected} bytes, got {}",
                        buf.len()
                    )));
                }
                break;
            }
        }
        buf.truncate(expected);

        if !(200..300).contains(&head.status) {
            return Err(EchoError::HttpStatus {
                status: head.status,
                reason: head.reason,
                retry_after: head.retry_after,
                body: String::from_utf8_lossy(&buf).into_owned(),
            });
        }
        Ok(buf)
    }
}

/// Parses a response head. Returns `None` if more bytes are needed.
fn parse_response_head(buf: &[u8]) -> Result<Option<ResponseHead>> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut res = httparse::Response::new(&mut headers);
    let head_len = match res.parse(buf) {
        Ok(httparse::Status::Complete(len)) => len,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(e) => return Err(EchoError::Http(format!("Invalid HTTP response: {e}"))),
    };

    let mut content_length = None;
    let mut retry_after = None;
    for header in res.headers.iter() {
        if header.name.eq_ignore_ascii_case("content-length") {
            let value = super::protocol::parse_content_length(header.value).ok_or_else(|| {
                EchoError::Http("Invalid Content-Length in HTTP response".to_string())
            })?;
            content_length = Some(value);
        } else if header.name.eq_ignore_ascii_case("retry-after") {
            retry_after = parse_retry_after(header.value);
        }
    }

    Ok(Some(ResponseHead {
        head_len,
        status: res.code.unwrap_or_default(),
        reason: res.reason.unwrap_or_default().to_string(),
        content_length,
        retry_after,
    }))
}

/// Parses a `Retry-After` value in delay-seconds form (`120`). HTTP-date
/// values and anything else give `None`.
fn parse_retry_after(value: &[u8]) -> Option<Duration> {
    let value = std::str::from_utf8(value).ok()?.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().map(Duration::from_secs)
}

#[async_trait]
impl EchoClient for HttpEchoClient {
    /// POSTs `data` and returns the response body.
    ///
    /// Errors on a non-2xx status ([`EchoError::HttpStatus`]), a response
    /// body larger than [`ClientConfig::max_response_size`], a truncated
    /// response (`UnexpectedEof`), or a timeout.
    async fn echo(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let mut stream = match self.pending.take() {
            Some(stream) => stream,
            None => Self::open(self.addr, &self.config).await?,
        };

        let mut request = format!(
            "POST / HTTP/1.1\r\nHost: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.addr,
            data.len()
        )
        .into_bytes();
        request.extend_from_slice(data);
        // The server echoes the body while it is still receiving it, so the
        // response is read concurrently with the request being written;
        // otherwise bodies larger than the socket buffers would deadlock.
        let write_timeout = self.config.write_timeout;
        let (mut reader, mut writer) = stream.split();
        let write = async {
            let send = async {
                writer.write_all(&request).await?;
                writer.flush().await
            };
            timeout(write_timeout, send)
                .await
                .map_err(|_| EchoError::Timeout("Write timeout".to_string()))?
                .map_err(EchoError::Tcp)
        };
        let read = self.read_response(&mut reader);
        tokio::pin!(write, read);

        let mut written = false;
        let mut write_error = None;
        loop {
            tokio::select! {
                result = &mut write, if !written => {
                    written = true;
                    match result {
                        Ok(()) => {}
                        Err(e @ EchoError::Timeout(_)) => return Err(e),
                        // The server may have answered early (e.g. 413) and
                        // stopped reading; its response is still worth reading.
                        Err(e) => write_error = Some(e),
                    }
                }
                result = &mut read => {
                    // An I/O error while reading is usually a consequence of
                    // the failed write; report the write error instead.
                    return match (result, write_error) {
                        (Err(EchoError::Tcp(_)), Some(write_error)) => Err(write_error),
                        (result, _) => result,
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_values() {
        assert_eq!(parse_retry_after(b"3"), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_after(b" 120 "), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after(b"0"), Some(Duration::ZERO));
        for bad in [
            &b""[..],
            b"-1",
            b"+1",
            b"1.5",
            b"soon",
            b"Wed, 21 Oct 2015 07:28:00 GMT",
            b"99999999999999999999999",
            b"\xff",
        ] {
            assert_eq!(parse_retry_after(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn response_head_carries_status_and_retry_after() {
        let raw = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nContent-Length: 0\r\n\r\n";
        let head = parse_response_head(raw).unwrap().unwrap();
        assert_eq!(head.status, 429);
        assert_eq!(head.reason, "Too Many Requests");
        assert_eq!(head.content_length, Some(0));
        assert_eq!(head.retry_after, Some(Duration::from_secs(2)));
        assert_eq!(head.head_len, raw.len());

        let head = parse_response_head(b"HTTP/1.1 200 OK\r\n\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(head.retry_after, None);
        assert!(
            parse_response_head(b"HTTP/1.1 200 OK\r\n")
                .unwrap()
                .is_none()
        );
    }
}
