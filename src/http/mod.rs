//! HTTP/1.1 echo server and client.
//!
//! [`HttpEchoServer`] echoes the **body** of each `POST` request back as the
//! body of a `200 OK` response. It is a deliberately small subset of
//! HTTP/1.1, meant for testing.
//!
//! # Server semantics
//!
//! * **One request per connection.** Every response carries
//!   `Connection: close`, and the server closes the connection after sending
//!   it. There is no keep-alive and no pipelining. Bytes after the first
//!   request are discarded.
//! * **Request head.** The server buffers the request line and headers until
//!   the blank line. They may arrive in any number of TCP segments, even one
//!   byte at a time.
//!   - A head longer than [`MAX_HEADER_BYTES`] (8 KiB) gets
//!     `431 Request Header Fields Too Large`.
//!   - More than [`MAX_HEADERS`] (32) headers, or any parse error, gets
//!     `400 Bad Request`.
//! * **Methods.** Only `POST` is accepted. Any other method gets
//!   `405 Method Not Allowed` with `Allow: POST`. The request path is ignored.
//! * **Body framing.** Only `Content-Length` is supported.
//!   - No `Content-Length` means an empty body.
//!   - An invalid `Content-Length` (not plain decimal digits, or several
//!     headers with different values) gets `400 Bad Request`.
//!   - A `Content-Length` above [`HttpConfig::max_body_size`] (default 1 MiB)
//!     gets `413 Content Too Large`. The body is not read.
//!   - Any `Transfer-Encoding` header (e.g. `chunked`) gets
//!     `501 Not Implemented`. Chunked bodies are out of scope; clients must
//!     send `Content-Length`.
//! * **`Expect: 100-continue`.** On HTTP/1.1 requests whose body has not fully
//!   arrived yet, the server sends `HTTP/1.1 100 Continue` before reading the
//!   body.
//! * **Successful response.**
//!   ```text
//!   HTTP/1.1 200 OK
//!   Server: <HttpConfig::server_name>          (omitted if None)
//!   Content-Type: <HttpConfig::default_content_type>  (omitted if None)
//!   Content-Length: <request body length>
//!   Connection: close
//!
//!   <request body, byte for byte>
//!   ```
//!   An empty body still gets a response, with `Content-Length: 0`. The body
//!   is streamed back while it is read, so it is never fully buffered.
//! * **Rate limits.** With [`HttpConfig::rate_limit`], a request over the
//!   limit gets `429 Too Many Requests` instead of the echo. With
//!   [`HttpConfig::accept_rate_limit`], a connection over the limit has its
//!   request head read and then gets the same `429`. The response carries
//!   `Retry-After: <seconds>` (rounded up, at least 1).
//! * **Connection limit.** A connection accepted while
//!   [`HttpConfig::max_connections`] connections are active has its request
//!   head read and then gets `503 Service Unavailable` (no `Retry-After`).
//! * **Error responses** use `Content-Type: text/plain; charset=utf-8` and a
//!   short explanation as the body. They also carry `Server` and
//!   `Connection: close`.
//! * Client protocol errors are logged at `debug` level, not reported as
//!   connection errors.
//!
//! # Client
//!
//! [`HttpEchoClient`] sends `POST / HTTP/1.1` with `Content-Length`. It
//! returns the response body and fails on any non-2xx status with
//! [`EchoError::HttpStatus`](crate::EchoError::HttpStatus), which carries the
//! status, body and `Retry-After` delay (see
//! [`EchoError::is_rate_limited`](crate::EchoError::is_rate_limited)). A
//! response cut short by the server closing the connection is an
//! [`UnexpectedEof`](std::io::ErrorKind::UnexpectedEof) error, never a
//! partial body.
//!
//! # Example
//!
//! ```
//! use echosrv::http::{HttpConfig, HttpEchoClient, HttpEchoServer};
//! use echosrv::{EchoClient, EchoServerTrait};
//!
//! #[tokio::main]
//! async fn main() -> echosrv::Result<()> {
//!     let config = HttpConfig {
//!         max_body_size: 64 * 1024,
//!         ..HttpConfig::default() // binds 127.0.0.1:0
//!     };
//!     let server = HttpEchoServer::new(config);
//!     let shutdown = server.shutdown_signal();
//!     let bound = server.bind().await?;
//!     let addr = *bound.local_addr().as_network().unwrap();
//!     let serving = tokio::spawn(bound.serve());
//!
//!     let mut client = HttpEchoClient::connect(addr).await?;
//!     assert_eq!(client.echo(b"hello").await?, b"hello");
//!
//!     shutdown.send(()).unwrap();
//!     serving.await.unwrap()
//! }
//! ```

pub mod client;
pub mod config;
pub mod protocol;
pub mod server;

#[cfg(test)]
mod tests;

pub use client::HttpEchoClient;
pub use config::HttpConfig;
pub use protocol::{
    DEFAULT_MAX_BODY_SIZE, HttpProtocol, HttpProtocolError, HttpStream, MAX_HEADER_BYTES,
    MAX_HEADERS,
};
pub use server::{BoundHttpServer, HttpEchoServer};
