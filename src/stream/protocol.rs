//! The [`StreamProtocol`] trait implemented by stream transports.

use super::config::StreamConfig;
use crate::network::fd_inheritance::FdInheritanceConfig;
use crate::network::{Address, LocalAddress};
use async_trait::async_trait;
use std::net::SocketAddr;
use std::time::Duration;

/// Why a server is rejecting a connection or request (see
/// [`StreamProtocol::reject`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// A request on an established connection exceeded the request rate
    /// limit ([`StreamConfig::rate_limit`]).
    RateLimited,
    /// A newly accepted connection exceeded the new-connection rate limit
    /// ([`StreamConfig::accept_rate_limit`]). Nothing has been read from the
    /// stream yet.
    ConnectionRateLimited,
}

/// A connection-oriented transport usable by [`StreamEchoServer`] and
/// [`Client`].
///
/// Implemented by [`TcpProtocol`](crate::tcp::TcpProtocol),
/// [`HttpProtocol`](crate::http::HttpProtocol) and
/// [`UnixStreamProtocol`](crate::unix::UnixStreamProtocol). The server calls
/// [`bind_with_inheritance`](Self::bind_with_inheritance), then repeatedly
/// [`accept`](Self::accept), and echoes each connection with
/// [`read`](Self::read), [`write`](Self::write) and [`flush`](Self::flush).
///
/// [`StreamEchoServer`]: crate::stream::StreamEchoServer
/// [`Client`]: crate::stream::Client
#[async_trait]
pub trait StreamProtocol: Send + Sync + 'static {
    /// Error type for this protocol.
    type Error: Send + Into<crate::EchoError>;
    /// Listening socket type.
    type Listener: Send + LocalAddress;
    /// Connected stream type.
    type Stream: Send;

    /// Binds a listener to the given configuration (server-side)
    ///
    /// Implementations should honor [`StreamConfig::bind_strategy`] and use the
    /// process-wide systemd descriptor pool
    /// ([`FdInheritanceConfig::from_systemd_env`]) for service-name lookups.
    async fn bind(config: &StreamConfig) -> std::result::Result<Self::Listener, Self::Error>;

    /// Binds a listener with explicit file descriptor inheritance configuration
    ///
    /// Generic servers call this method. `fd_config` is the pool used for
    /// service-name lookups when the strategy is
    /// [`InheritOrBind`](crate::network::BindStrategy::InheritOrBind).
    ///
    /// The default implementation ignores `fd_config` and calls [`bind`](Self::bind).
    async fn bind_with_inheritance(
        config: &StreamConfig,
        _fd_config: &FdInheritanceConfig,
    ) -> std::result::Result<Self::Listener, Self::Error> {
        // Default implementation ignores FD inheritance and uses standard binding
        // Protocols that support inheritance should override this method
        Self::bind(config).await
    }

    /// Accepts a new connection from the listener (server side).
    ///
    /// The returned address is only used for logging; protocols without IP
    /// peers (Unix sockets) return a placeholder.
    async fn accept(
        listener: &mut Self::Listener,
    ) -> std::result::Result<(Self::Stream, SocketAddr), Self::Error>;

    /// Connects to a server at the given address (client-side)
    async fn connect(addr: SocketAddr) -> std::result::Result<Self::Stream, Self::Error>;

    /// Connects to a server at a unified [`Address`] (client-side).
    ///
    /// The default implementation handles [`Address::Network`] via
    /// [`connect`](Self::connect) and rejects [`Address::Unix`] with an
    /// [`Unsupported`](std::io::ErrorKind::Unsupported) I/O error. Unix
    /// protocols override it.
    async fn connect_address(addr: &Address) -> std::result::Result<Self::Stream, Self::Error> {
        match addr {
            Address::Network(addr) => Self::connect(*addr).await,
            Address::Unix(path) => Err(Self::map_io_error(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "protocol does not support Unix socket address {}",
                    path.display()
                ),
            ))),
        }
    }

    /// Reads data from a stream
    async fn read(
        stream: &mut Self::Stream,
        buffer: &mut [u8],
    ) -> std::result::Result<usize, Self::Error>;

    /// Writes data to a stream
    async fn write(stream: &mut Self::Stream, data: &[u8]) -> std::result::Result<(), Self::Error>;

    /// Flushes a stream
    async fn flush(stream: &mut Self::Stream) -> std::result::Result<(), Self::Error>;

    /// Maps a standard IO error to this protocol's error type
    fn map_io_error(err: std::io::Error) -> Self::Error;

    /// Whether the protocol frames requests itself (server side).
    ///
    /// * `false` (the default): the stream is a plain byte stream. For the
    ///   request rate limit ([`StreamConfig::rate_limit`]) the server counts
    ///   every non-empty [`read`](Self::read) as one request.
    /// * `true`: the server calls [`begin_request`](Self::begin_request) once
    ///   at the start of each connection and counts that as the request. The
    ///   connection then serves this one request (until `read` returns `0`).
    const FRAMED_REQUESTS: bool = false;

    /// Reads the start of the request on a new connection (server side,
    /// only called when [`FRAMED_REQUESTS`](Self::FRAMED_REQUESTS) is `true`).
    ///
    /// Returns `Ok(true)` when a request is ready to be served, and
    /// `Ok(false)` when there is nothing to serve, e.g. the peer closed the
    /// connection or the protocol already answered with an error. The server
    /// applies [`StreamConfig::read_timeout`] to the call.
    ///
    /// The default implementation returns `Ok(true)` without reading.
    async fn begin_request(_stream: &mut Self::Stream) -> std::result::Result<bool, Self::Error> {
        Ok(true)
    }

    /// Tells the peer it was rejected by a rate limit; the server closes the
    /// stream right after this returns (server side).
    ///
    /// `retry_after` is the earliest time at which the request would have
    /// been admitted. The server bounds the call with a timeout, counts the
    /// rejection in [`ServerStats`](crate::ServerStats) and logs it at
    /// `debug` level, so implementations need not do either.
    ///
    /// The default implementation does nothing, so the connection is simply
    /// closed. [`TcpProtocol`](crate::tcp::TcpProtocol) resets the
    /// connection, and [`HttpProtocol`](crate::http::HttpProtocol) answers
    /// `429 Too Many Requests`.
    async fn reject(
        _stream: &mut Self::Stream,
        _reason: RejectReason,
        _retry_after: Duration,
    ) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
}
