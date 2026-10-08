//! Async echo servers and clients built on Tokio, intended for testing and
//! development.
//!
//! Every server sends back exactly what it receives: bytes on a stream
//! connection, each datagram to its sender, or the body of each HTTP `POST`
//! request. Matching clients send a payload and return the echo.
//!
//! The crate also builds an `echosrv` command-line binary that runs one of
//! these servers.
//!
//! # Protocols
//!
//! | Protocol      | Server                     | Client                     | Config                 |
//! |---------------|----------------------------|----------------------------|------------------------|
//! | TCP           | [`TcpEchoServer`]          | [`TcpEchoClient`]          | [`TcpConfig`]          |
//! | UDP           | [`UdpEchoServer`]          | [`UdpEchoClient`]          | [`UdpConfig`]          |
//! | HTTP/1.1      | [`HttpEchoServer`]         | [`HttpEchoClient`]         | [`HttpConfig`]         |
//! | Unix stream   | [`UnixStreamEchoServer`]   | [`UnixStreamEchoClient`]   | [`UnixStreamConfig`]   |
//! | Unix datagram | [`UnixDatagramEchoServer`] | [`UnixDatagramEchoClient`] | [`UnixDatagramConfig`] |
//!
//! TCP and UDP are type aliases over the generic [`StreamEchoServer`] and
//! [`DatagramEchoServer`]. Their constructors take a [`StreamConfig`] /
//! [`DatagramConfig`], so convert the protocol config with `.into()`. The
//! HTTP and Unix servers are thin wrappers that take their own config
//! directly. All servers implement [`EchoServerTrait`] and all clients
//! implement [`EchoClient`]. New protocols plug into the generic servers by
//! implementing [`stream::StreamProtocol`] or [`datagram::DatagramProtocol`].
//!
//! **Platform support:** Unix-like systems only (Linux, macOS, BSD). The crate
//! fails to compile on other targets.
//!
//! # Quickstart
//!
//! Bind to port 0, read the actual address, echo once and shut down:
//!
//! ```
//! use echosrv::{EchoClient, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
//!
//! #[tokio::main]
//! async fn main() -> echosrv::Result<()> {
//!     // The default config binds 127.0.0.1:0 (an ephemeral port).
//!     let server = TcpEchoServer::new(TcpConfig::default().into());
//!     let shutdown = server.shutdown_signal();
//!
//!     let bound = server.bind().await?;
//!     let addr = *bound.local_addr().as_network().expect("TCP address");
//!     let serving = tokio::spawn(bound.serve());
//!
//!     let mut client = TcpEchoClient::connect(addr).await?;
//!     assert_eq!(client.echo_string("hello").await?, "hello");
//!
//!     // Graceful shutdown: stop accepting, cancel and await connections.
//!     shutdown.send(()).expect("server is running");
//!     serving.await.expect("server task panicked")?;
//!     Ok(())
//! }
//! ```
//!
//! [`EchoServerTrait::run`] combines `bind` and `serve` when the address is
//! known in advance. Signal handling (Ctrl-C, `SIGTERM`) is left to the
//! caller; forward it through [`EchoServerTrait::shutdown_signal`].
//!
//! # HTTP
//!
//! [`HttpEchoServer`] implements a small subset of HTTP/1.1: it answers each
//! `POST` with `200 OK` and the request body, byte for byte. Other methods
//! get `405 Method Not Allowed`. Bodies are framed by `Content-Length` only
//! (`Transfer-Encoding` gets `501`), limited by [`HttpConfig::max_body_size`]
//! (`413` above it), and every connection serves one request and then closes.
//! See the [`http`] module for the exact semantics and limits.
//!
//! # Rate limiting
//!
//! Every server config has an optional `rate_limit`, and the stream configs
//! (TCP, HTTP, Unix stream) also an `accept_rate_limit` for new connections.
//! Both take a [`RateLimitConfig`] (sustained rate per second plus burst) and
//! apply to the whole server. Traffic over a limit is rejected, never delayed:
//!
//! | Server             | Over `rate_limit`                          | Over `accept_rate_limit`           |
//! |--------------------|--------------------------------------------|------------------------------------|
//! | TCP                | connection reset (RST)                     | connection reset (RST)             |
//! | HTTP               | `429 Too Many Requests` with `Retry-After` | request read, then the same `429`  |
//! | Unix stream        | connection closed                          | connection closed                  |
//! | UDP, Unix datagram | datagram dropped                           | n/a                                |
//!
//! For TCP and Unix streams every chunk read counts as one request; for HTTP
//! every request does. Rejections are counted in [`ServerStats`], available
//! from `stats()` on every server, and logged at `debug` level only. The
//! primitives, [`Gcra`] and [`TokenBucket`], are in [`rate_limit`].
//!
//! ```
//! use echosrv::{EchoServerTrait, RateLimitConfig, TcpConfig, TcpEchoServer};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> echosrv::Result<()> {
//! let config = TcpConfig::default()
//!     .with_rate_limit(RateLimitConfig::new(1_000, 100)) // 1000 reads/s, bursts of 100
//!     .with_accept_rate_limit(RateLimitConfig::new(50, 10)); // 50 new connections/s
//! let server = TcpEchoServer::new(config.into());
//! let stats = server.stats();
//! let bound = server.bind().await?;
//! # drop(bound);
//! assert_eq!(stats.rejected_connections(), 0);
//! # Ok(())
//! # }
//! ```
//!
//! # Socket inheritance and systemd socket activation
//!
//! Instead of binding, a server can use a socket created by a parent process
//! (systemd, a process manager, a launcher doing blue/green restarts). This is
//! opt-in through the config's `bind_strategy`, a
//! [`BindStrategy`](network::BindStrategy):
//!
//! * [`Bind`](network::BindStrategy::Bind): bind a new socket. This is what
//!   the default configs do; nothing is inherited unless you ask for it.
//! * [`Inherit`](network::BindStrategy::Inherit): use the given
//!   [`InheritedFd`](network::InheritedFd), and fail if it cannot be used.
//! * [`InheritOrBind`](network::BindStrategy::InheritOrBind): use an inherited
//!   descriptor if one is available, otherwise bind a fallback target.
//!   `with_fd_inheritance` on every config sets this up (for example
//!   [`TcpConfig::with_fd_inheritance`] and
//!   [`UnixStreamConfig::with_fd_inheritance`], which also takes the fallback
//!   path).
//!
//! With `InheritOrBind` and no explicit descriptor, the server looks in the
//! process-wide systemd pool
//! ([`FdInheritanceConfig::from_systemd_env`](network::FdInheritanceConfig::from_systemd_env)).
//! The pool contains the descriptors passed via `LISTEN_FDS` /
//! `LISTEN_FDNAMES`, and is empty unless `LISTEN_PID` equals the current
//! process ID. A server takes the descriptor whose name equals its
//! `service_name` (systemd's `FileDescriptorName=`); if there is none and
//! exactly one descriptor was passed in total, it takes that one whatever its
//! name. Otherwise it binds the fallback. Each descriptor is owned by at most
//! one server, and inherited descriptors are checked for the right socket
//! type, address family and (for stream servers) listening state. See
//! [`network::fd_inheritance`] for details.
//!
//! Inheriting a descriptor explicitly:
//!
//! ```
//! use echosrv::network::{BindStrategy, InheritedFd};
//! use echosrv::{EchoServerTrait, TcpConfig, TcpEchoServer};
//! use std::os::fd::OwnedFd;
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> echosrv::Result<()> {
//! // Stands in for a listening socket passed by a parent process.
//! let parent = std::net::TcpListener::bind("127.0.0.1:0")?;
//! let parent_addr = parent.local_addr()?;
//!
//! let config = TcpConfig {
//!     bind_strategy: Some(BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(parent)))),
//!     ..TcpConfig::default()
//! };
//! let server = TcpEchoServer::new(config.into());
//! let bound = server.bind().await?;
//! assert_eq!(bound.local_addr().as_network(), Some(&parent_addr));
//! # Ok(())
//! # }
//! ```
//!
//! # Errors
//!
//! Fallible library functions return [`Result<T>`](Result), an alias for
//! `std::result::Result<T, EchoError>`. [`EchoError`] has one variant per
//! failure domain: socket I/O ([`Tcp`](EchoError::Tcp),
//! [`Udp`](EchoError::Udp), [`Unix`](EchoError::Unix)), invalid configuration
//! ([`Config`](EchoError::Config)), socket inheritance
//! ([`FdInheritance`](EchoError::FdInheritance)), client timeouts
//! ([`Timeout`](EchoError::Timeout)), invalid UTF-8 in
//! [`EchoClient::echo_string`] ([`Utf8`](EchoError::Utf8)), unsupported
//! operations ([`Unsupported`](EchoError::Unsupported)), malformed HTTP
//! ([`Http`](EchoError::Http)) and non-2xx HTTP responses seen by
//! [`HttpEchoClient`] ([`HttpStatus`](EchoError::HttpStatus)).
//! [`EchoError::into_io_error`] converts any of them into a
//! [`std::io::Error`].
//!
//! Clients never return a partial echo as success: a connection closed early
//! is an [`UnexpectedEof`](std::io::ErrorKind::UnexpectedEof) I/O error, a
//! reply that did not arrive in time is a [`Timeout`](EchoError::Timeout), and
//! a reply larger than the client's limit is a [`Config`](EchoError::Config)
//! error. To classify failures (for example in a load tester), use
//! [`EchoError::io_error_kind`], [`EchoError::is_rate_limited`] and
//! [`EchoError::retry_after`]:
//!
//! ```
//! use echosrv::EchoError;
//! use std::io::ErrorKind;
//!
//! fn describe(err: &EchoError) -> &'static str {
//!     if err.is_rate_limited() {
//!         return "rate limited";
//!     }
//!     match (err, err.io_error_kind()) {
//!         (EchoError::Timeout(_), _) => "timeout",
//!         (_, Some(ErrorKind::ConnectionRefused)) => "refused",
//!         (_, Some(ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof)) => "reset",
//!         (_, Some(ErrorKind::NotFound)) => "no such socket",
//!         _ => "other",
//!     }
//! }
//!
//! let refused = EchoError::Tcp(ErrorKind::ConnectionRefused.into());
//! assert_eq!(describe(&refused), "refused");
//! ```

#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(not(unix))]
compile_error!("echosrv currently supports Unix-like platforms only (Linux, macOS, BSD)");

use crate::http::protocol::HttpProtocolError;
use std::time::Duration;
use thiserror::Error;

/// Error type for the echosrv library.
///
/// See the [crate-level overview](crate#errors).
#[derive(Error, Debug)]
pub enum EchoError {
    /// I/O error on a TCP (or HTTP) socket: bind, accept, connect, read, write.
    ///
    /// This is also the variant produced by the blanket
    /// `From<std::io::Error>` conversion (and therefore by `?` on an
    /// [`std::io::Error`]).
    #[error("TCP error: {0}")]
    Tcp(#[from] std::io::Error),

    /// I/O error on a UDP socket: bind, send, receive.
    #[error("UDP error: {0}")]
    Udp(std::io::Error),

    /// I/O error on a Unix domain socket: bind, accept, connect, read, write.
    #[error("Unix domain socket error: {0}")]
    Unix(std::io::Error),

    /// Invalid configuration or argument, e.g. a zero `buffer_size`, a bind
    /// target of the wrong kind, an unparsable [`Address`], or a payload or
    /// response larger than a client's limit.
    #[error("Configuration error: {0}")]
    Config(String),

    /// An inherited file descriptor could not be used: already consumed, not a
    /// socket, or the wrong socket type, address family or listening state.
    #[error("FD inheritance error: {0}")]
    FdInheritance(String),

    /// A client connect, read or write did not complete in time.
    #[error("Timeout error: {0}")]
    Timeout(String),

    /// An echoed response was not valid UTF-8 (from [`EchoClient::echo_string`]).
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    /// The operation is not supported, e.g. connecting a Unix client to a
    /// network address or the HTTP client to a Unix socket.
    #[error("Unsupported operation: {0}")]
    Unsupported(String),

    /// HTTP error: a malformed response seen by [`HttpEchoClient`], or a
    /// converted [`HttpProtocolError`]. Non-2xx responses are
    /// [`HttpStatus`](EchoError::HttpStatus).
    #[error("HTTP error: {0}")]
    Http(String),

    /// A complete HTTP response with a non-2xx status, seen by
    /// [`HttpEchoClient`]. The server is reachable; it declined the request
    /// (for example `429 Too Many Requests` from a rate limit).
    #[error("HTTP error: HTTP {status} {reason}: {body}")]
    HttpStatus {
        /// Status code, e.g. `429`.
        status: u16,
        /// Reason phrase, e.g. `Too Many Requests` (may be empty).
        reason: String,
        /// The `Retry-After` header in delay-seconds form, if present.
        /// HTTP-date values are not parsed and give `None`.
        retry_after: Option<Duration>,
        /// Response body, decoded as UTF-8 (lossily).
        body: String,
    },
}

impl EchoError {
    /// Converts this error into a [`std::io::Error`].
    ///
    /// I/O-backed variants (`Tcp`, `Udp`, `Unix`) return the underlying error
    /// unchanged; every other variant is wrapped with [`std::io::Error::other`].
    /// This is useful when an `EchoError` must cross an API that only speaks
    /// `std::io::Error` (for example a protocol-specific error type).
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::EchoError;
    ///
    /// let err = EchoError::Config("bad".into()).into_io_error();
    /// assert_eq!(err.kind(), std::io::ErrorKind::Other);
    /// ```
    pub fn into_io_error(self) -> std::io::Error {
        match self {
            EchoError::Tcp(e) | EchoError::Udp(e) | EchoError::Unix(e) => e,
            other => std::io::Error::other(other.to_string()),
        }
    }

    /// The [`std::io::ErrorKind`] of an I/O-backed error (`Tcp`, `Udp`,
    /// `Unix`), or `None` for every other variant.
    ///
    /// Useful to classify client failures: `ConnectionRefused` (nothing
    /// listening), `NotFound` (no Unix socket at the path),
    /// `ConnectionReset`/`BrokenPipe` (connection dropped), and
    /// `UnexpectedEof` (the server closed the connection before the whole
    /// echo arrived). Timeouts are [`EchoError::Timeout`], not an I/O kind.
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::EchoError;
    /// use std::io::ErrorKind;
    ///
    /// let err = EchoError::Unix(ErrorKind::NotFound.into());
    /// assert_eq!(err.io_error_kind(), Some(ErrorKind::NotFound));
    /// assert_eq!(EchoError::Timeout("read".into()).io_error_kind(), None);
    /// ```
    pub fn io_error_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            EchoError::Tcp(e) | EchoError::Udp(e) | EchoError::Unix(e) => Some(e.kind()),
            _ => None,
        }
    }

    /// Whether the server rejected the request because of a rate limit,
    /// i.e. an HTTP `429 Too Many Requests` ([`EchoError::HttpStatus`]).
    ///
    /// The other servers signal rate limiting without a status: TCP resets
    /// the connection, a Unix stream closes it and datagram servers drop the
    /// datagram. Those surface as I/O errors or timeouts.
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::EchoError;
    /// use std::time::Duration;
    ///
    /// let err = EchoError::HttpStatus {
    ///     status: 429,
    ///     reason: "Too Many Requests".into(),
    ///     retry_after: Some(Duration::from_secs(1)),
    ///     body: String::new(),
    /// };
    /// assert!(err.is_rate_limited());
    /// assert!(!EchoError::Http("bad".into()).is_rate_limited());
    /// ```
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, EchoError::HttpStatus { status: 429, .. })
    }

    /// The server's `Retry-After` delay for an [`EchoError::HttpStatus`]
    /// that carried one, otherwise `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::EchoError;
    /// use std::time::Duration;
    ///
    /// let err = EchoError::HttpStatus {
    ///     status: 503,
    ///     reason: "Service Unavailable".into(),
    ///     retry_after: Some(Duration::from_secs(2)),
    ///     body: String::new(),
    /// };
    /// assert_eq!(err.retry_after(), Some(Duration::from_secs(2)));
    /// ```
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            EchoError::HttpStatus { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

impl From<HttpProtocolError> for EchoError {
    fn from(err: HttpProtocolError) -> Self {
        match err {
            HttpProtocolError::Io(e) => EchoError::Tcp(e),
            HttpProtocolError::Bind(e) => e,
            other => EchoError::Http(other.to_string()),
        }
    }
}

/// Result type for the echosrv library, using [`EchoError`].
pub type Result<T> = std::result::Result<T, EchoError>;

pub mod common;
pub mod datagram;
pub mod defaults;
pub mod http;
pub mod network;
pub mod rate_limit;
pub mod stream;
pub mod tcp;
pub mod udp;
#[cfg(unix)]
pub mod unix;

// Re-exports of the main types.
pub use common::{EchoClient, EchoServerTrait, ServerStats};
pub use datagram::{DatagramConfig, DatagramEchoClient, DatagramEchoServer};
pub use http::{HttpConfig, HttpEchoClient, HttpEchoServer, HttpProtocol};
pub use network::Address;
pub use rate_limit::{Gcra, RateLimitConfig, RateLimitError, RateLimited, TokenBucket};
pub use stream::{Client as StreamClient, StreamConfig, StreamEchoServer};
pub use tcp::{TcpConfig, TcpEchoClient, TcpEchoServer};
pub use udp::{UdpConfig, UdpEchoClient, UdpEchoServer};
#[cfg(unix)]
pub use unix::{
    UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixStreamConfig,
    UnixStreamEchoClient, UnixStreamEchoServer,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn io_err() -> io::Error {
        io::Error::new(io::ErrorKind::ConnectionRefused, "refused")
    }

    fn status_error(status: u16, retry_after: Option<Duration>) -> EchoError {
        EchoError::HttpStatus {
            status,
            reason: String::new(),
            retry_after,
            body: String::new(),
        }
    }

    #[test]
    fn io_error_kind_only_for_io_variants() {
        for err in [
            EchoError::Tcp(io_err()),
            EchoError::Udp(io_err()),
            EchoError::Unix(io_err()),
        ] {
            assert_eq!(err.io_error_kind(), Some(io::ErrorKind::ConnectionRefused));
        }
        for err in [
            EchoError::Config("c".into()),
            EchoError::Timeout("t".into()),
            EchoError::Http("h".into()),
            status_error(502, None),
        ] {
            assert_eq!(err.io_error_kind(), None, "{err:?}");
        }
    }

    #[test]
    fn rate_limited_and_retry_after() {
        let secs = Some(Duration::from_secs(3));
        assert!(status_error(429, None).is_rate_limited());
        assert!(status_error(429, secs).is_rate_limited());
        assert!(!status_error(503, secs).is_rate_limited());
        assert!(!EchoError::Http("429".into()).is_rate_limited());
        assert!(!EchoError::Tcp(io_err()).is_rate_limited());

        assert_eq!(status_error(429, secs).retry_after(), secs);
        assert_eq!(status_error(503, secs).retry_after(), secs);
        assert_eq!(status_error(429, None).retry_after(), None);
        assert_eq!(EchoError::Timeout("t".into()).retry_after(), None);
    }

    #[test]
    fn display_every_variant() {
        let utf8 = String::from_utf8(vec![0xff]).unwrap_err();
        let utf8_msg = utf8.to_string();
        let cases = [
            (EchoError::Tcp(io_err()), "TCP error: refused".to_string()),
            (EchoError::Udp(io_err()), "UDP error: refused".to_string()),
            (
                EchoError::Unix(io_err()),
                "Unix domain socket error: refused".to_string(),
            ),
            (
                EchoError::Config("bad".into()),
                "Configuration error: bad".to_string(),
            ),
            (
                EchoError::FdInheritance("fd".into()),
                "FD inheritance error: fd".to_string(),
            ),
            (
                EchoError::Timeout("slow".into()),
                "Timeout error: slow".to_string(),
            ),
            (EchoError::Utf8(utf8), format!("UTF-8 error: {utf8_msg}")),
            (
                EchoError::Unsupported("nope".into()),
                "Unsupported operation: nope".to_string(),
            ),
            (EchoError::Http("400".into()), "HTTP error: 400".to_string()),
            (
                EchoError::HttpStatus {
                    status: 429,
                    reason: "Too Many Requests".into(),
                    retry_after: None,
                    body: "slow down".into(),
                },
                "HTTP error: HTTP 429 Too Many Requests: slow down".to_string(),
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn io_backed_variants_expose_source() {
        use std::error::Error as _;
        for err in [
            EchoError::Tcp(io_err()),
            EchoError::Udp(io_err()),
            EchoError::Unix(io_err()),
        ] {
            // Tcp uses #[from] (which implies #[source]); Udp/Unix do not
            // declare a source, so only check Display there.
            if matches!(err, EchoError::Tcp(_)) {
                assert!(err.source().is_some());
            }
            assert!(err.to_string().ends_with("refused"));
        }
        assert!(EchoError::Config("x".into()).source().is_none());
    }

    #[test]
    fn from_io_error_is_tcp() {
        let err: EchoError = io_err().into();
        match err {
            EchoError::Tcp(e) => assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused),
            other => panic!("expected Tcp, got {other:?}"),
        }
    }

    #[test]
    fn from_utf8_error() {
        let err: EchoError = String::from_utf8(vec![0xc3]).unwrap_err().into();
        assert!(matches!(err, EchoError::Utf8(_)));
    }

    #[test]
    fn from_http_protocol_error_maps_io_to_tcp_and_rest_to_http() {
        let err: EchoError = HttpProtocolError::Io(io_err()).into();
        assert!(
            matches!(err, EchoError::Tcp(ref e) if e.kind() == io::ErrorKind::ConnectionRefused)
        );

        let cases = [
            (
                HttpProtocolError::HttpParse("bad line".into()),
                "HTTP error: HTTP parsing error: bad line",
            ),
            (
                HttpProtocolError::InvalidRequest("too long".into()),
                "HTTP error: Invalid request: too long",
            ),
            (
                HttpProtocolError::IncompleteRequest,
                "HTTP error: Incomplete request",
            ),
        ];
        for (http, expected) in cases {
            let err: EchoError = http.into();
            assert!(matches!(err, EchoError::Http(_)), "{err:?}");
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn from_http_protocol_error_unwraps_bind_errors() {
        let err: EchoError = HttpProtocolError::Bind(EchoError::FdInheritance("f".into())).into();
        assert!(
            matches!(err, EchoError::FdInheritance(ref m) if m == "f"),
            "{err:?}"
        );

        let err: EchoError = HttpProtocolError::Bind(EchoError::Config("c".into())).into();
        assert!(
            matches!(err, EchoError::Config(ref m) if m == "c"),
            "{err:?}"
        );

        let err: EchoError = HttpProtocolError::Bind(EchoError::Tcp(io_err())).into();
        assert!(
            matches!(err, EchoError::Tcp(ref e) if e.kind() == io::ErrorKind::ConnectionRefused)
        );
    }

    #[test]
    fn into_io_error_unwraps_io_variants() {
        for err in [
            EchoError::Tcp(io_err()),
            EchoError::Udp(io_err()),
            EchoError::Unix(io_err()),
        ] {
            let io = err.into_io_error();
            assert_eq!(io.kind(), io::ErrorKind::ConnectionRefused);
            assert_eq!(io.to_string(), "refused");
        }
    }

    #[test]
    fn into_io_error_wraps_other_variants() {
        let cases = [
            EchoError::Config("c".into()),
            EchoError::FdInheritance("f".into()),
            EchoError::Timeout("t".into()),
            EchoError::Utf8(String::from_utf8(vec![0xff]).unwrap_err()),
            EchoError::Unsupported("u".into()),
            EchoError::Http("h".into()),
            status_error(500, None),
        ];
        for err in cases {
            let message = err.to_string();
            let io = err.into_io_error();
            assert_eq!(io.kind(), io::ErrorKind::Other);
            assert_eq!(io.to_string(), message);
        }
    }
}
