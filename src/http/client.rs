//! A minimal HTTP/1.1 client for the HTTP echo server.

use super::protocol::MAX_HEADERS;
use crate::common::EchoClient;
use crate::network::Address;
use crate::stream::ClientConfig;
use crate::{EchoError, Result};
use async_trait::async_trait;
use std::net::SocketAddr;
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
/// A non-2xx status is returned as an error that includes the status code
/// and response body. Interim `1xx` responses are skipped.
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
    /// connect, read and write timeouts and the maximum response body size.
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

    async fn read_some<R: AsyncRead + Unpin>(
        &self,
        stream: &mut R,
        buf: &mut Vec<u8>,
    ) -> Result<usize> {
        let mut chunk = [0u8; 8192];
        let n = timeout(self.config.read_timeout, stream.read(&mut chunk))
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
            if self.read_some(stream, buf).await? == 0 {
                return Err(EchoError::Http(
                    "Connection closed before the HTTP response head was complete".to_string(),
                ));
            }
        }
    }

    /// Reads the response and returns its body. Errors on a non-2xx status,
    /// a body larger than `max_response_size`, or a truncated body.
    async fn read_response<R: AsyncRead + Unpin>(&self, stream: &mut R) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        let head = self.read_head(stream, &mut buf).await?;
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
            if self.read_some(stream, &mut buf).await? == 0 {
                if head.content_length.is_some() {
                    return Err(EchoError::Http(format!(
                        "HTTP response body truncated: expected {expected} bytes, got {}",
                        buf.len()
                    )));
                }
                break;
            }
        }
        buf.truncate(expected);

        if !(200..300).contains(&head.status) {
            return Err(EchoError::Http(format!(
                "HTTP {} {}: {}",
                head.status,
                head.reason,
                String::from_utf8_lossy(&buf)
            )));
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
    for header in res.headers.iter() {
        if header.name.eq_ignore_ascii_case("content-length") {
            let value = super::protocol::parse_content_length(header.value).ok_or_else(|| {
                EchoError::Http("Invalid Content-Length in HTTP response".to_string())
            })?;
            content_length = Some(value);
        }
    }

    Ok(Some(ResponseHead {
        head_len,
        status: res.code.unwrap_or_default(),
        reason: res.reason.unwrap_or_default().to_string(),
        content_length,
    }))
}

#[async_trait]
impl EchoClient for HttpEchoClient {
    /// POSTs `data` and returns the response body.
    ///
    /// Errors on a non-2xx status, a response body larger than
    /// [`ClientConfig::max_response_size`], a truncated response, or a timeout.
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
