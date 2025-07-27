use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio::time::timeout;

/// Resource limits for echo servers
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum request/message size in bytes
    pub max_request_size: usize,
    /// Maximum concurrent connections (for stream protocols)
    pub max_concurrent_connections: usize,
    /// Maximum requests per second per client (rate limiting)
    pub max_requests_per_second: Option<u32>,
    /// Connection timeout
    pub connection_timeout: Duration,
    /// Maximum idle time before closing connection
    pub max_idle_time: Duration,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_request_size: 1024 * 1024, // 1MB
            max_concurrent_connections: 100,
            max_requests_per_second: Some(100), // 100 RPS per client
            connection_timeout: Duration::from_secs(30),
            max_idle_time: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Rate limiter for controlling request frequency
#[derive(Debug)]
pub struct RateLimiter {
    permits: Arc<Semaphore>,
    refill_rate: u32,
    last_refill: Arc<std::sync::Mutex<Instant>>,
}

impl RateLimiter {
    pub fn new(requests_per_second: u32) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(requests_per_second as usize)),
            refill_rate: requests_per_second,
            last_refill: Arc::new(std::sync::Mutex::new(Instant::now())),
        }
    }

    /// Attempt to acquire a permit for a request
    pub async fn acquire(&self) -> Result<(), RateLimitError> {
        // Try to refill permits if enough time has passed
        self.try_refill();

        // Try to acquire a permit with timeout
        match timeout(Duration::from_millis(100), self.permits.acquire()).await {
            Ok(Ok(_permit)) => Ok(()),
            Ok(Err(_)) => Err(RateLimitError::Closed),
            Err(_) => Err(RateLimitError::Exceeded),
        }
    }

    fn try_refill(&self) {
        let now = Instant::now();
        if let Ok(mut last_refill) = self.last_refill.try_lock() {
            let elapsed = now.duration_since(*last_refill);
            if elapsed >= Duration::from_secs(1) {
                // Refill permits based on elapsed time
                let permits_to_add = (elapsed.as_secs() as u32 * self.refill_rate) as usize;
                let current_permits = self.permits.available_permits();
                let max_permits = self.refill_rate as usize;

                if current_permits < max_permits {
                    let actual_add = std::cmp::min(permits_to_add, max_permits - current_permits);
                    self.permits.add_permits(actual_add);
                }

                *last_refill = now;
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("Rate limit exceeded")]
    Exceeded,
    #[error("Rate limiter closed")]
    Closed,
}

/// Connection tracking and management
#[derive(Debug)]
pub struct ConnectionTracker {
    active_connections: AtomicUsize,
    total_connections: AtomicU64,
    connection_semaphore: Arc<Semaphore>,
    limits: ResourceLimits,
}

impl ConnectionTracker {
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            active_connections: AtomicUsize::new(0),
            total_connections: AtomicU64::new(0),
            connection_semaphore: Arc::new(Semaphore::new(limits.max_concurrent_connections)),
            limits,
        }
    }

    /// Attempt to acquire a connection slot
    pub async fn acquire_connection(&self) -> Result<ConnectionGuard, ConnectionError> {
        // Try to acquire a permit for the connection
        let permit = timeout(Duration::from_secs(1), self.connection_semaphore.acquire())
            .await
            .map_err(|_| ConnectionError::Timeout)?
            .map_err(|_| ConnectionError::Closed)?;

        let active = self.active_connections.fetch_add(1, Ordering::SeqCst) + 1;
        let total = self.total_connections.fetch_add(1, Ordering::SeqCst) + 1;

        tracing::info!(
            active_connections = active,
            total_connections = total,
            "Connection acquired"
        );

        Ok(ConnectionGuard {
            _permit: permit,
            tracker: self,
            start_time: Instant::now(),
        })
    }

    /// Get current metrics
    pub fn metrics(&self) -> ConnectionMetrics {
        ConnectionMetrics {
            active_connections: self.active_connections.load(Ordering::SeqCst),
            total_connections: self.total_connections.load(Ordering::SeqCst),
            available_slots: self.connection_semaphore.available_permits(),
            max_connections: self.limits.max_concurrent_connections,
        }
    }
}

/// RAII guard for connection tracking
pub struct ConnectionGuard<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
    tracker: &'a ConnectionTracker,
    start_time: Instant,
}

impl<'a> Drop for ConnectionGuard<'a> {
    fn drop(&mut self) {
        let active = self
            .tracker
            .active_connections
            .fetch_sub(1, Ordering::SeqCst)
            - 1;
        let duration = self.start_time.elapsed();

        tracing::info!(
            active_connections = active,
            connection_duration_ms = duration.as_millis(),
            "Connection released"
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Connection limit reached, timeout waiting for slot")]
    Timeout,
    #[error("Connection tracker closed")]
    Closed,
}

/// Connection metrics for monitoring
#[derive(Debug, Clone)]
pub struct ConnectionMetrics {
    pub active_connections: usize,
    pub total_connections: u64,
    pub available_slots: usize,
    pub max_connections: usize,
}

/// Size validator for requests/messages
pub struct SizeValidator {
    max_size: usize,
}

impl SizeValidator {
    pub fn new(max_size: usize) -> Self {
        Self { max_size }
    }

    pub fn validate_size(&self, size: usize) -> Result<(), SizeError> {
        if size > self.max_size {
            Err(SizeError::TooLarge {
                actual: size,
                max: self.max_size,
            })
        } else {
            Ok(())
        }
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SizeError {
    #[error("Request too large: {actual} bytes, maximum allowed: {max} bytes")]
    TooLarge { actual: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter() {
        let _limiter = RateLimiter::new(1); // 1 request per second

        // Basic test - just verify creation works
        // Rate limiting logic would need more sophisticated testing with timing
        // This is left as a future improvement
    }

    #[tokio::test]
    async fn test_connection_tracker() {
        let limits = ResourceLimits {
            max_concurrent_connections: 2,
            ..Default::default()
        };
        let tracker = ConnectionTracker::new(limits);

        // Acquire two connections
        let _guard1 = tracker.acquire_connection().await.unwrap();
        let _guard2 = tracker.acquire_connection().await.unwrap();

        // Third connection should timeout
        assert!(matches!(
            tracker.acquire_connection().await,
            Err(ConnectionError::Timeout)
        ));

        // Drop one guard and try again
        drop(_guard1);
        let _guard3 = tracker.acquire_connection().await.unwrap();
    }

    #[test]
    fn test_size_validator() {
        let validator = SizeValidator::new(100);

        assert!(validator.validate_size(50).is_ok());
        assert!(validator.validate_size(100).is_ok());
        assert!(validator.validate_size(101).is_err());
    }
}
