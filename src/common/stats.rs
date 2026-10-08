//! Server-side counters, readable while a server is running.

use std::sync::atomic::{AtomicU64, Ordering};

/// Counters for traffic a server refused instead of echoing.
///
/// Every server has one. Get it with `stats()` on the server (for example
/// [`StreamEchoServer::stats`](crate::StreamEchoServer::stats)) or on the
/// bound server. The handle is an `Arc` shared with the running server, so it
/// can be read at any time, also after the server stopped. The counters only
/// grow; they are never reset.
///
/// # Examples
///
/// ```
/// use echosrv::{TcpConfig, TcpEchoServer};
///
/// let server = TcpEchoServer::new(TcpConfig::default().into());
/// let stats = server.stats();
/// assert_eq!(stats.rejected_requests(), 0);
/// assert_eq!(stats.rejected_connections(), 0);
/// assert_eq!(stats.dropped_rate_limited(), 0);
/// ```
#[derive(Debug, Default)]
pub struct ServerStats {
    rejected_requests: AtomicU64,
    rejected_connections: AtomicU64,
    dropped_rate_limited: AtomicU64,
}

impl ServerStats {
    /// Requests rejected by the request rate limit (`rate_limit`) on stream
    /// servers (TCP, HTTP, Unix stream).
    pub fn rejected_requests(&self) -> u64 {
        self.rejected_requests.load(Ordering::Relaxed)
    }

    /// Connections rejected by the new-connection rate limit
    /// (`accept_rate_limit`) on stream servers.
    ///
    /// Connections closed because `max_connections` was reached are not
    /// counted here.
    pub fn rejected_connections(&self) -> u64 {
        self.rejected_connections.load(Ordering::Relaxed)
    }

    /// Datagrams dropped by the request rate limit (`rate_limit`) on datagram
    /// servers (UDP, Unix datagram).
    pub fn dropped_rate_limited(&self) -> u64 {
        self.dropped_rate_limited.load(Ordering::Relaxed)
    }

    /// Increments [`rejected_requests`](Self::rejected_requests) and returns
    /// the new total.
    pub(crate) fn inc_rejected_requests(&self) -> u64 {
        self.rejected_requests.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Increments [`rejected_connections`](Self::rejected_connections) and
    /// returns the new total.
    pub(crate) fn inc_rejected_connections(&self) -> u64 {
        self.rejected_connections.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Increments [`dropped_rate_limited`](Self::dropped_rate_limited) and
    /// returns the new total.
    pub(crate) fn inc_dropped_rate_limited(&self) -> u64 {
        self.dropped_rate_limited.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_start_at_zero_and_increment_independently() {
        let stats = ServerStats::default();
        assert_eq!(stats.inc_rejected_requests(), 1);
        assert_eq!(stats.inc_rejected_requests(), 2);
        assert_eq!(stats.inc_rejected_connections(), 1);
        assert_eq!(stats.inc_dropped_rate_limited(), 1);
        assert_eq!(stats.rejected_requests(), 2);
        assert_eq!(stats.rejected_connections(), 1);
        assert_eq!(stats.dropped_rate_limited(), 1);
    }
}
