//! Server lifecycle helpers shared by all server implementations:
//! shutdown signalling, connection accounting, rate limiting and accept-error
//! backoff.

use crate::rate_limit::{Gcra, RateLimitConfig, RateLimited};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;

/// Delay applied after a failed `accept()` (e.g. `EMFILE`) so the server does
/// not spin in a hot loop while the condition persists.
pub(crate) const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Upper bound on the time a server spends telling a peer it was rejected by
/// a rate limit (see `StreamProtocol::reject`) before closing the connection.
pub(crate) const REJECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum number of connection rejections a stream server runs at once
/// (see `StreamProtocol::reject`). These do not take `max_connections`
/// slots, so a flood of rejected connections cannot lock out admitted ones.
/// Connections rejected while this many are in flight are closed right away.
pub(crate) const MAX_PENDING_REJECTIONS: usize = 32;

/// The rate limiters of one server, shared by all its connections.
///
/// Each limiter is created once per server (in `new()`), so the limit is
/// global to the server and survives repeated `bind()`/`serve()` calls.
#[derive(Debug, Default)]
pub(crate) struct RateLimiters {
    /// Request limit (`rate_limit`): requests or datagrams.
    requests: Option<Gcra>,
    /// New-connection limit (`accept_rate_limit`), stream servers only.
    connections: Option<Gcra>,
}

impl RateLimiters {
    pub(crate) fn new(
        requests: Option<RateLimitConfig>,
        connections: Option<RateLimitConfig>,
    ) -> Self {
        Self {
            requests: requests.map(Gcra::new),
            connections: connections.map(Gcra::new),
        }
    }

    /// Admits one request (or datagram) under the request limit.
    pub(crate) fn check_request(&self) -> Result<(), RateLimited> {
        self.requests.as_ref().map_or(Ok(()), Gcra::check)
    }

    /// Admits one new connection under the accept limit.
    pub(crate) fn check_connection(&self) -> Result<(), RateLimited> {
        self.connections.as_ref().map_or(Ok(()), Gcra::check)
    }
}

/// Shutdown channel whose first receiver is created together with the sender.
///
/// `tokio::sync::broadcast` only delivers messages to receivers that exist at
/// send time. Subscribing at construction means a shutdown sent *before*
/// `run()` starts is not lost.
pub(crate) struct ShutdownSignal {
    tx: broadcast::Sender<()>,
    pending: Mutex<Option<broadcast::Receiver<()>>>,
}

impl ShutdownSignal {
    pub(crate) fn new() -> Self {
        let (tx, rx) = broadcast::channel(1);
        Self {
            tx,
            pending: Mutex::new(Some(rx)),
        }
    }

    /// A sender that triggers shutdown when `send(())` is called.
    pub(crate) fn sender(&self) -> broadcast::Sender<()> {
        self.tx.clone()
    }

    /// A receiver for one server run. The first call returns the receiver
    /// created at construction (so early signals are observed); later calls
    /// subscribe afresh.
    pub(crate) fn receiver(&self) -> broadcast::Receiver<()> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_else(|| self.tx.subscribe())
    }
}

/// Resolves when a shutdown message arrives.
///
/// If every sender has been dropped no shutdown can ever be requested, so this
/// future then stays pending forever instead of resolving spuriously.
pub(crate) async fn wait_for_shutdown(rx: &mut broadcast::Receiver<()>) {
    match rx.recv().await {
        Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
        Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
    }
}

/// RAII slot in a bounded connection counter.
///
/// Acquisition uses a single `fetch_add` (no load-then-add race) and rolls back
/// if the limit was exceeded. The slot is released on drop, including when the
/// connection task panics.
#[derive(Debug)]
pub(crate) struct ConnectionGuard {
    counter: Arc<AtomicUsize>,
}

impl ConnectionGuard {
    /// Reserves a slot, or returns `None` if `max` connections are active.
    pub(crate) fn try_acquire(counter: &Arc<AtomicUsize>, max: usize) -> Option<Self> {
        let previous = counter.fetch_add(1, Ordering::AcqRel);
        if previous >= max {
            counter.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self {
            counter: Arc::clone(counter),
        })
    }

    /// Number of active connections including this one.
    pub(crate) fn active(&self) -> usize {
        self.counter.load(Ordering::Acquire)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_enforces_limit_and_releases() {
        let counter = Arc::new(AtomicUsize::new(0));
        let a = ConnectionGuard::try_acquire(&counter, 2).unwrap();
        assert_eq!(a.active(), 1);
        let b = ConnectionGuard::try_acquire(&counter, 2).unwrap();
        assert_eq!(b.active(), 2);
        // A failed acquire rolls its increment back.
        assert!(ConnectionGuard::try_acquire(&counter, 2).is_none());
        assert!(ConnectionGuard::try_acquire(&counter, 2).is_none());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        drop(a);
        assert_eq!(b.active(), 1);
        let c = ConnectionGuard::try_acquire(&counter, 2).unwrap();
        drop((b, c));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn guard_with_zero_limit_never_acquires() {
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            assert!(ConnectionGuard::try_acquire(&counter, 0).is_none());
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn guard_holders_never_exceed_limit() {
        const MAX: usize = 3;
        let counter = Arc::new(AtomicUsize::new(0));
        let holders = Arc::new(AtomicUsize::new(0));
        let violations = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let counter = Arc::clone(&counter);
                let holders = Arc::clone(&holders);
                let violations = Arc::clone(&violations);
                std::thread::spawn(move || {
                    for _ in 0..2_000 {
                        if let Some(guard) = ConnectionGuard::try_acquire(&counter, MAX) {
                            if holders.fetch_add(1, Ordering::SeqCst) >= MAX {
                                violations.fetch_add(1, Ordering::SeqCst);
                            }
                            std::hint::spin_loop();
                            holders.fetch_sub(1, Ordering::SeqCst);
                            drop(guard);
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(violations.load(Ordering::SeqCst), 0);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn guard_released_on_panic() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        let result = std::panic::catch_unwind(move || {
            let _guard = ConnectionGuard::try_acquire(&c, 1).unwrap();
            panic!("boom");
        });
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert!(ConnectionGuard::try_acquire(&counter, 1).is_some());
    }

    #[tokio::test]
    async fn guard_released_when_task_panics() {
        let counter = Arc::new(AtomicUsize::new(0));
        let guard = ConnectionGuard::try_acquire(&counter, 1).unwrap();
        let joined = tokio::spawn(async move {
            let _guard = guard;
            panic!("connection task failed");
        })
        .await;
        assert!(joined.unwrap_err().is_panic());
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rate_limiters_are_independent_and_optional() {
        let unlimited = RateLimiters::default();
        for _ in 0..1_000 {
            assert!(unlimited.check_request().is_ok());
            assert!(unlimited.check_connection().is_ok());
        }

        let limited = RateLimiters::new(
            Some(RateLimitConfig::new(1, 2)),
            Some(RateLimitConfig::new(1, 1)),
        );
        assert!(limited.check_request().is_ok());
        assert!(limited.check_request().is_ok());
        let denied = limited.check_request().unwrap_err();
        assert!(denied.retry_after > Duration::ZERO);
        // The connection limit has its own budget.
        assert!(limited.check_connection().is_ok());
        assert!(limited.check_connection().is_err());

        let requests_only = RateLimiters::new(Some(RateLimitConfig::new(1, 1)), None);
        assert!(requests_only.check_request().is_ok());
        assert!(requests_only.check_request().is_err());
        for _ in 0..10 {
            assert!(requests_only.check_connection().is_ok());
        }
    }

    #[tokio::test]
    async fn shutdown_sent_before_receiver_taken_is_observed() {
        let signal = ShutdownSignal::new();
        signal.sender().send(()).unwrap();
        let mut rx = signal.receiver();
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(&mut rx))
            .await
            .expect("early shutdown signal was lost");
    }

    #[tokio::test]
    async fn later_receivers_subscribe_afresh() {
        let signal = ShutdownSignal::new();
        let mut first = signal.receiver();
        signal.sender().send(()).unwrap();
        wait_for_shutdown(&mut first).await;

        // A second run gets a new subscription: the old message is not replayed.
        let mut second = signal.receiver();
        assert!(matches!(
            second.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        signal.sender().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(&mut second))
            .await
            .expect("second receiver missed shutdown");
    }

    #[tokio::test]
    async fn lagged_receiver_counts_as_shutdown() {
        let signal = ShutdownSignal::new();
        let mut rx = signal.receiver();
        let tx = signal.sender();
        // Capacity is 1: the second send makes the receiver lag.
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), wait_for_shutdown(&mut rx))
            .await
            .expect("lagged receiver did not observe shutdown");
    }

    #[tokio::test]
    async fn closed_channel_never_resolves() {
        let (tx, mut rx) = broadcast::channel::<()>(1);
        drop(tx);
        tokio::time::pause();
        let waited =
            tokio::time::timeout(Duration::from_secs(3600), wait_for_shutdown(&mut rx)).await;
        assert!(waited.is_err(), "closed channel must not trigger shutdown");
    }
}
