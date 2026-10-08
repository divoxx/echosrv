//! Rate-limiting primitives.
//!
//! Two complementary algorithms are provided:
//!
//! - [`Gcra`]: a lock-free *policer* based on the Generic Cell Rate Algorithm.
//!   It never waits; [`Gcra::check`] either admits the event or returns how
//!   long the caller would have to wait, which suits servers that reject
//!   over-limit requests and connections.
//! - [`TokenBucket`]: an async *shaper*. [`TokenBucket::acquire`] blocks until a
//!   token is available, so clients can pace their outgoing traffic.
//!
//! Both are configured through [`RateLimitConfig`] and are `Send + Sync`, so
//! they can be shared between tasks behind an [`Arc`](std::sync::Arc).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Rate-limit settings shared by [`Gcra`] and [`TokenBucket`].
///
/// `rate_per_sec` is the sustained rate; `burst` is how many events may happen
/// back to back (for [`Gcra`]) or the bucket capacity (for [`TokenBucket`]).
/// Zero values are treated as `1`.
///
/// # Examples
///
/// ```
/// use echosrv::rate_limit::RateLimitConfig;
///
/// let cfg = RateLimitConfig::new(100, 10);
/// assert_eq!(cfg.rate_per_sec, 100);
/// assert_eq!(cfg.burst, 10);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// Sustained rate, in events per second.
    pub rate_per_sec: u32,
    /// Maximum number of events allowed back to back.
    pub burst: u32,
}

impl RateLimitConfig {
    /// Creates a new configuration.
    pub const fn new(rate_per_sec: u32, burst: u32) -> Self {
        Self {
            rate_per_sec,
            burst,
        }
    }

    fn rate(&self) -> u64 {
        u64::from(self.rate_per_sec.max(1))
    }

    fn burst(&self) -> u64 {
        u64::from(self.burst.max(1))
    }
}

/// Returned by [`Gcra::check`] when an event exceeds the configured rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rate limit exceeded, retry after {retry_after:?}")]
pub struct RateLimited {
    /// Earliest time from now at which the same event would be admitted.
    pub retry_after: Duration,
}

/// Rate-limit errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RateLimitError {
    /// The rate limit was exceeded.
    #[error("rate limit exceeded, retry after {retry_after:?}")]
    Exceeded {
        /// Earliest time from now at which the request would be admitted.
        retry_after: Duration,
    },
}

impl From<RateLimited> for RateLimitError {
    fn from(err: RateLimited) -> Self {
        RateLimitError::Exceeded {
            retry_after: err.retry_after,
        }
    }
}

/// Lock-free GCRA (Generic Cell Rate Algorithm) policer.
///
/// With emission interval `T = 1s / rate` and tolerance `τ = T * burst`, an
/// event arriving at `now` is admitted iff `max(tat, now) + T - now <= τ`,
/// where `tat` is the theoretical arrival time of the next event. This admits
/// exactly `burst` back-to-back events from idle and then one every `T`.
///
/// # Examples
///
/// ```
/// use echosrv::rate_limit::{Gcra, RateLimitConfig};
///
/// let limiter = Gcra::new(RateLimitConfig::new(10, 2));
/// assert!(limiter.check().is_ok());
/// assert!(limiter.check().is_ok());
/// let denied = limiter.check().unwrap_err();
/// assert!(denied.retry_after.as_millis() > 0);
/// ```
#[derive(Debug)]
pub struct Gcra {
    base: Instant,
    emission_interval_ns: u64,
    tolerance_ns: u64,
    /// Theoretical arrival time, in nanoseconds since `base`.
    tat: AtomicU64,
}

impl Gcra {
    /// Creates a new policer that starts idle (a full burst is available).
    pub fn new(config: RateLimitConfig) -> Self {
        let emission_interval_ns = (NANOS_PER_SEC / config.rate()).max(1);
        Self {
            base: Instant::now(),
            emission_interval_ns,
            tolerance_ns: emission_interval_ns.saturating_mul(config.burst()),
            tat: AtomicU64::new(0),
        }
    }

    /// Checks (and, if admitted, records) one event at the current time.
    pub fn check(&self) -> Result<(), RateLimited> {
        self.check_at(Instant::now())
    }

    /// Same as [`check`](Self::check) with an explicit timestamp.
    ///
    /// Timestamps earlier than the policer's creation are treated as its
    /// creation time.
    pub fn check_at(&self, now: Instant) -> Result<(), RateLimited> {
        let now_ns =
            u64::try_from(now.saturating_duration_since(self.base).as_nanos()).unwrap_or(u64::MAX);
        let mut tat = self.tat.load(Ordering::Acquire);
        loop {
            let new_tat = tat.max(now_ns).saturating_add(self.emission_interval_ns);
            let ahead = new_tat - now_ns;
            if ahead > self.tolerance_ns {
                return Err(RateLimited {
                    retry_after: Duration::from_nanos(ahead - self.tolerance_ns),
                });
            }
            match self
                .tat
                .compare_exchange_weak(tat, new_tat, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(()),
                Err(actual) => tat = actual,
            }
        }
    }
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last: tokio::time::Instant,
}

impl BucketState {
    /// Adds the tokens accrued since `last`, capped at `capacity`.
    fn refill_at(&mut self, now: tokio::time::Instant, rate: f64, capacity: f64) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rate).min(capacity);
        self.last = self.last.max(now);
    }

    /// Refills, then takes one token or returns how long until one is available.
    fn try_take_at(
        &mut self,
        now: tokio::time::Instant,
        rate: f64,
        capacity: f64,
    ) -> Result<(), Duration> {
        // Tolerate float rounding so a sleep of exactly the computed wait
        // yields a token instead of a second (timer-granularity) sleep.
        const EPSILON: f64 = 1e-9;
        self.refill_at(now, rate, capacity);
        if self.tokens >= 1.0 - EPSILON {
            self.tokens = (self.tokens - 1.0).max(0.0);
            Ok(())
        } else {
            // Round up to whole nanoseconds: a truncated wait would refill
            // just short of a token, forcing a second sleep.
            let wait_ns = ((1.0 - self.tokens) / rate * 1e9).ceil();
            Err(Duration::from_nanos(wait_ns as u64))
        }
    }
}

/// Async token-bucket shaper.
///
/// The bucket starts full (`burst` tokens) and refills at `rate_per_sec`.
/// [`acquire`](Self::acquire) waits until a token is available rather than
/// failing. Waiters are served in FIFO order. A `burst` of 1 gives smooth
/// pacing.
///
/// # Examples
///
/// ```
/// use echosrv::rate_limit::{RateLimitConfig, TokenBucket};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let bucket = TokenBucket::new(RateLimitConfig::new(1000, 5));
/// for _ in 0..5 {
///     bucket.acquire().await; // the initial burst does not wait
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct TokenBucket {
    rate: f64,
    capacity: f64,
    state: tokio::sync::Mutex<BucketState>,
}

impl TokenBucket {
    /// Creates a full bucket.
    pub fn new(config: RateLimitConfig) -> Self {
        let capacity = config.burst() as f64;
        Self {
            rate: config.rate() as f64,
            capacity,
            state: tokio::sync::Mutex::new(BucketState {
                tokens: capacity,
                last: tokio::time::Instant::now(),
            }),
        }
    }

    /// Waits until a token is available and takes it.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe: a token is only consumed when the future
    /// completes, so dropping it (e.g. in `tokio::select!`) takes nothing.
    pub async fn acquire(&self) {
        // Holding the (fair) lock while sleeping queues waiters in FIFO order
        // and avoids a thundering herd when a token becomes available.
        let mut state = self.state.lock().await;
        loop {
            let now = tokio::time::Instant::now();
            match state.try_take_at(now, self.rate, self.capacity) {
                Ok(()) => return,
                Err(wait) => {
                    let target = now + wait;
                    tokio::time::sleep_until(target).await;
                    // Take the token as of `target`, not the (later) wake-up
                    // time: timer overshoot is then credited to the next
                    // caller instead of being cut off by the capacity cap,
                    // which would otherwise lower the effective rate.
                    if state.try_take_at(target, self.rate, self.capacity).is_ok() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn gcra_allows_exactly_burst_then_rate() {
        // rate 10/s => T = 100ms; burst 3.
        let g = Gcra::new(RateLimitConfig::new(10, 3));
        let t0 = g.base;
        for _ in 0..3 {
            assert!(g.check_at(t0).is_ok());
        }
        assert!(g.check_at(t0).is_err());
        assert!(g.check_at(t0 + ms(99)).is_err());
        // One new slot every 100ms.
        assert!(g.check_at(t0 + ms(100)).is_ok());
        assert!(g.check_at(t0 + ms(100)).is_err());
        assert!(g.check_at(t0 + ms(200)).is_ok());
        assert!(g.check_at(t0 + ms(200)).is_err());
        // After a long idle period, the full burst (and no more) is available.
        let later = t0 + Duration::from_secs(10);
        for _ in 0..3 {
            assert!(g.check_at(later).is_ok());
        }
        assert!(g.check_at(later).is_err());
    }

    #[test]
    fn gcra_retry_after_is_exact() {
        let g = Gcra::new(RateLimitConfig::new(5, 5)); // T = 200ms
        let t0 = g.base;
        for _ in 0..5 {
            g.check_at(t0).unwrap();
        }
        assert_eq!(g.check_at(t0).unwrap_err().retry_after, ms(200));
        assert_eq!(g.check_at(t0 + ms(50)).unwrap_err().retry_after, ms(150));
        // Retrying exactly after `retry_after` succeeds.
        assert!(g.check_at(t0 + ms(200)).is_ok());
        // A denied check does not consume capacity.
        assert_eq!(g.check_at(t0 + ms(200)).unwrap_err().retry_after, ms(200));
    }

    #[test]
    fn gcra_zero_config_is_clamped() {
        let g = Gcra::new(RateLimitConfig::new(0, 0));
        let t0 = g.base;
        assert!(g.check_at(t0).is_ok());
        assert_eq!(
            g.check_at(t0).unwrap_err().retry_after,
            Duration::from_secs(1)
        );
    }

    #[test]
    fn gcra_concurrent_checks_never_exceed_bound() {
        let g = Arc::new(Gcra::new(RateLimitConfig::new(1, 50)));
        let t0 = g.base;
        let admitted = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let g = g.clone();
                let admitted = admitted.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        if g.check_at(t0).is_ok() {
                            admitted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(admitted.load(Ordering::Relaxed), 50);
    }

    #[test]
    fn rate_limit_error_from_rate_limited() {
        let err: RateLimitError = RateLimited { retry_after: ms(7) }.into();
        assert_eq!(err, RateLimitError::Exceeded { retry_after: ms(7) });
    }

    #[test]
    fn bucket_refills_and_caps_at_capacity() {
        let t0 = tokio::time::Instant::now();
        let mut s = BucketState {
            tokens: 2.0,
            last: t0,
        };
        // Drain.
        assert!(s.try_take_at(t0, 10.0, 2.0).is_ok());
        assert!(s.try_take_at(t0, 10.0, 2.0).is_ok());
        let wait = s.try_take_at(t0, 10.0, 2.0).unwrap_err();
        assert!(
            wait.abs_diff(ms(100)) < Duration::from_micros(1),
            "wait = {wait:?}"
        );
        // Half a token after 50ms.
        s.refill_at(t0 + ms(50), 10.0, 2.0);
        assert!((s.tokens - 0.5).abs() < 1e-9);
        let wait = s.try_take_at(t0 + ms(50), 10.0, 2.0).unwrap_err();
        assert!(
            wait.abs_diff(ms(50)) < Duration::from_micros(1),
            "wait = {wait:?}"
        );
        // Long idle: capped at capacity.
        s.refill_at(t0 + Duration::from_secs(60), 10.0, 2.0);
        assert_eq!(s.tokens, 2.0);
        // Time going backwards does not panic or add tokens.
        s.refill_at(t0, 10.0, 2.0);
        assert_eq!(s.tokens, 2.0);
    }

    #[tokio::test(start_paused = true)]
    async fn bucket_acquire_waits_one_over_rate() {
        let bucket = TokenBucket::new(RateLimitConfig::new(10, 1));
        let start = tokio::time::Instant::now();
        bucket.acquire().await; // initial token
        assert_eq!(start.elapsed(), Duration::ZERO);
        bucket.acquire().await;
        let e = start.elapsed();
        assert!(e >= ms(100) && e < ms(101), "elapsed = {e:?}");
        for _ in 0..10 {
            bucket.acquire().await;
        }
        let e = start.elapsed();
        assert!(e >= ms(1100) && e < ms(1102), "elapsed = {e:?}");
    }

    #[tokio::test]
    async fn bucket_rate_survives_timer_overshoot() {
        // 1ms waits are at the timer's granularity, so wake-ups overshoot.
        // The overshoot must be credited to later acquires rather than lost
        // to the capacity cap, keeping the long-run rate at ~1000/s.
        let bucket = TokenBucket::new(RateLimitConfig::new(1000, 1));
        let start = tokio::time::Instant::now();
        for _ in 0..300 {
            bucket.acquire().await;
        }
        let e = start.elapsed();
        assert!(
            e >= ms(290) && e < ms(550),
            "300 acquires at 1000/s took {e:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn bucket_acquire_is_cancel_safe() {
        let bucket = TokenBucket::new(RateLimitConfig::new(10, 1));
        bucket.acquire().await;
        // Cancel a pending acquire part-way through its wait.
        tokio::select! {
            _ = bucket.acquire() => panic!("should not have acquired"),
            _ = tokio::time::sleep(ms(50)) => {}
        }
        // The token that accrues at 100ms is still available.
        let start = tokio::time::Instant::now();
        bucket.acquire().await;
        let e = start.elapsed();
        assert!(e >= ms(50) && e < ms(51), "elapsed = {e:?}");
    }
}
