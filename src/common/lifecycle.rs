//! Server lifecycle helpers shared by all server implementations:
//! shutdown signalling, connection accounting and accept-error backoff.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;

/// Delay applied after a failed `accept()` (e.g. `EMFILE`) so the server does
/// not spin in a hot loop while the condition persists.
pub(crate) const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

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
        let b = ConnectionGuard::try_acquire(&counter, 2).unwrap();
        assert!(ConnectionGuard::try_acquire(&counter, 2).is_none());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        drop(a);
        let c = ConnectionGuard::try_acquire(&counter, 2).unwrap();
        drop((b, c));
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
}
