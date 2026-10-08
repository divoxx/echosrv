//! Pure statistics: error classification, latency windows, outage tracking
//! and the aggregator task that turns worker samples into reports.

use echosrv::EchoError;
use hdrhistogram::Histogram;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

/// Highest latency tracked by the histograms (larger values saturate).
const MAX_LATENCY_US: u64 = 60_000_000;
/// Maximum number of outage windows kept in the summary.
const MAX_OUTAGE_WINDOWS: usize = 100;

/// Where in a worker's loop an error happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Creating the client (connecting / binding).
    Connect,
    /// Sending the payload and waiting for the echo.
    Echo,
}

/// Error classes reported by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The server actively refused the connection (nothing listening on the
    /// port, or a stale Unix socket file).
    ConnectRefused,
    /// The server could not be reached otherwise: a missing Unix socket
    /// path, or any other failure while connecting.
    ConnectFailed,
    /// The connection was reset, aborted or closed before the whole echo
    /// arrived.
    Reset,
    /// Connect, read or write timed out.
    Timeout,
    /// The server rejected the request because of rate limiting (HTTP 429).
    RateLimited,
    /// The echoed payload differs from what was sent.
    Mismatch,
    /// The client machine ran out of local ports (`EADDRNOTAVAIL`), from too
    /// many new connections per second (see [`crate::runner::SAFE_CONN_RATE`]).
    /// The run stops when it happens.
    PortsExhausted,
    /// Anything else.
    Other,
}

impl ErrorKind {
    /// All kinds, in index order.
    pub const ALL: [ErrorKind; ErrorKind::COUNT] = [
        ErrorKind::ConnectRefused,
        ErrorKind::ConnectFailed,
        ErrorKind::Reset,
        ErrorKind::Timeout,
        ErrorKind::RateLimited,
        ErrorKind::Mismatch,
        ErrorKind::PortsExhausted,
        ErrorKind::Other,
    ];

    /// Number of kinds.
    pub const COUNT: usize = 8;

    /// Position of this kind in [`ErrorKind::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Short, stable name used in reports and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::ConnectRefused => "connect_refused",
            ErrorKind::ConnectFailed => "connect_failed",
            ErrorKind::Reset => "reset",
            ErrorKind::Timeout => "timeout",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::Mismatch => "mismatch",
            ErrorKind::PortsExhausted => "ports_exhausted",
            ErrorKind::Other => "other",
        }
    }

    /// Whether this error means the server is unavailable: it could not be
    /// reached, or it dropped the connection or did not answer in time.
    /// Rate limiting, mismatches and other errors are answers from a live
    /// server, and port exhaustion is a problem of the client machine, so
    /// none of them counts towards an outage.
    pub fn is_outage(self) -> bool {
        matches!(
            self,
            ErrorKind::ConnectRefused
                | ErrorKind::ConnectFailed
                | ErrorKind::Reset
                | ErrorKind::Timeout
        )
    }
}

/// Maps a library error to an [`ErrorKind`].
///
/// Refusals and missing Unix socket paths are classified the same in both
/// phases: datagram clients only find out when they send.
pub fn classify(phase: Phase, err: &EchoError) -> ErrorKind {
    if err.is_rate_limited() {
        return ErrorKind::RateLimited;
    }
    if let EchoError::Timeout(_) = err {
        return ErrorKind::Timeout;
    }
    match err.io_error_kind() {
        Some(io::ErrorKind::ConnectionRefused) => ErrorKind::ConnectRefused,
        Some(io::ErrorKind::AddrNotAvailable) => ErrorKind::PortsExhausted,
        Some(io::ErrorKind::NotFound) => ErrorKind::ConnectFailed,
        Some(
            io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::NotConnected,
        ) => ErrorKind::Reset,
        Some(io::ErrorKind::TimedOut) => ErrorKind::Timeout,
        _ if phase == Phase::Connect => ErrorKind::ConnectFailed,
        _ => ErrorKind::Other,
    }
}

/// Result of one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Err(ErrorKind),
}

/// One attempt, as reported by a worker.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// When the attempt completed.
    pub at: Instant,
    /// How long the attempt took (connect time for connect failures).
    pub latency: Duration,
    pub outcome: Outcome,
}

fn new_histogram() -> Histogram<u64> {
    Histogram::new_with_bounds(1, MAX_LATENCY_US, 3).expect("valid histogram bounds")
}

/// Counters and a latency histogram (successes only) for a period of time.
#[derive(Debug, Clone)]
pub struct Window {
    pub count: u64,
    pub ok: u64,
    pub errors: [u64; ErrorKind::COUNT],
    /// Latency of successful requests, in microseconds.
    pub hist: Histogram<u64>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            count: 0,
            ok: 0,
            errors: [0; ErrorKind::COUNT],
            hist: new_histogram(),
        }
    }
}

impl Window {
    pub fn record(&mut self, sample: &Sample) {
        self.count += 1;
        match sample.outcome {
            Outcome::Ok => {
                self.ok += 1;
                let us = u64::try_from(sample.latency.as_micros()).unwrap_or(u64::MAX);
                self.hist.saturating_record(us.clamp(1, MAX_LATENCY_US));
            }
            Outcome::Err(kind) => self.errors[kind.index()] += 1,
        }
    }

    pub fn merge(&mut self, other: &Window) {
        self.count += other.count;
        self.ok += other.ok;
        for (a, b) in self.errors.iter_mut().zip(other.errors) {
            *a += b;
        }
        self.hist
            .add(&other.hist)
            .expect("histograms share the same bounds");
    }

    pub fn error_count(&self) -> u64 {
        self.errors.iter().sum()
    }

    pub fn errors_of(&self, kind: ErrorKind) -> u64 {
        self.errors[kind.index()]
    }

    /// Latency at quantile `q` (0.0..=1.0) of successful requests.
    pub fn percentile(&self, q: f64) -> Option<Duration> {
        (!self.hist.is_empty()).then(|| Duration::from_micros(self.hist.value_at_quantile(q)))
    }

    /// Non-zero error counts by kind name.
    pub fn nonzero_errors(&self) -> BTreeMap<&'static str, u64> {
        ErrorKind::ALL
            .iter()
            .filter(|k| self.errors_of(**k) > 0)
            .map(|k| (k.as_str(), self.errors_of(*k)))
            .collect()
    }

    fn latency_summary(&self) -> Option<LatencySummary> {
        if self.hist.is_empty() {
            return None;
        }
        let ms = |us: u64| us as f64 / 1000.0;
        Some(LatencySummary {
            min_ms: ms(self.hist.min()),
            mean_ms: self.hist.mean() / 1000.0,
            p50_ms: ms(self.hist.value_at_quantile(0.50)),
            p90_ms: ms(self.hist.value_at_quantile(0.90)),
            p99_ms: ms(self.hist.value_at_quantile(0.99)),
            p999_ms: ms(self.hist.value_at_quantile(0.999)),
            max_ms: ms(self.hist.max()),
        })
    }
}

/// A period during which every attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct OutageWindow {
    /// Seconds since the start of the run.
    pub start_s: f64,
    pub duration_ms: f64,
    /// Number of failed attempts during the outage.
    pub errors: u64,
    /// Still open when the run ended.
    pub ongoing: bool,
}

#[derive(Debug, Clone, Copy)]
struct OpenOutage {
    start: Instant,
    last_error: Instant,
    errors: u64,
}

/// A change in outage state, reported live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutageEvent {
    Started { at_s: f64, kind: ErrorKind },
    Ended { at_s: f64, window: OutageWindow },
}

/// Detects outages: consecutive outage errors ([`ErrorKind::is_outage`])
/// across all workers open an outage and the next success closes it. The outage spans
/// from the completion of the first failed attempt to the completion of the
/// first successful one.
///
/// Samples can arrive slightly out of order (workers report after the
/// fact), so a success that completed before the outage started does not
/// close it.
#[derive(Debug)]
pub struct OutageTracker {
    base: Instant,
    current: Option<OpenOutage>,
    count: u64,
    total: Duration,
    longest: Duration,
    windows: Vec<OutageWindow>,
}

impl OutageTracker {
    pub fn new(base: Instant) -> Self {
        Self {
            base,
            current: None,
            count: 0,
            total: Duration::ZERO,
            longest: Duration::ZERO,
            windows: Vec::new(),
        }
    }

    pub fn in_outage(&self) -> bool {
        self.current.is_some()
    }

    fn offset_s(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.base).as_secs_f64()
    }

    /// Feeds one attempt that started at `started` and completed at `at`;
    /// returns an event if the outage state changed.
    ///
    /// A success only ends an outage if it *started* after the outage did.
    /// When a server goes down, requests already in flight on other
    /// connections can still complete; they say nothing about the server
    /// after the first failure and would otherwise split one outage into
    /// several zero-length ones.
    pub fn observe(
        &mut self,
        started: Instant,
        at: Instant,
        outcome: Outcome,
    ) -> Option<OutageEvent> {
        match outcome {
            Outcome::Err(kind) if !kind.is_outage() => None,
            Outcome::Err(kind) => match &mut self.current {
                Some(open) => {
                    open.errors += 1;
                    open.last_error = open.last_error.max(at);
                    None
                }
                None => {
                    self.current = Some(OpenOutage {
                        start: at,
                        last_error: at,
                        errors: 1,
                    });
                    Some(OutageEvent::Started {
                        at_s: self.offset_s(at),
                        kind,
                    })
                }
            },
            Outcome::Ok => {
                if self.current.is_some_and(|open| started < open.start) {
                    return None;
                }
                let open = self.current.take()?;
                let window = self.close(open, at, false);
                Some(OutageEvent::Ended {
                    at_s: self.offset_s(at),
                    window,
                })
            }
        }
    }

    fn close(&mut self, open: OpenOutage, end: Instant, ongoing: bool) -> OutageWindow {
        let duration = end.saturating_duration_since(open.start);
        self.count += 1;
        self.total += duration;
        self.longest = self.longest.max(duration);
        let window = OutageWindow {
            start_s: self.offset_s(open.start),
            duration_ms: duration.as_secs_f64() * 1000.0,
            errors: open.errors,
            ongoing,
        };
        if self.windows.len() < MAX_OUTAGE_WINDOWS {
            self.windows.push(window);
        }
        window
    }

    /// Closes any open outage at `end` (marked `ongoing`) and summarizes.
    pub fn finish(mut self, end: Instant) -> OutageSummary {
        let ongoing = match self.current.take() {
            Some(open) => {
                let end = end.max(open.last_error);
                self.close(open, end, true);
                true
            }
            None => false,
        };
        OutageSummary {
            count: self.count,
            total_ms: self.total.as_secs_f64() * 1000.0,
            longest_ms: self.longest.as_secs_f64() * 1000.0,
            ongoing,
            windows_truncated: self.count > self.windows.len() as u64,
            windows: self.windows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutageSummary {
    pub count: u64,
    pub total_ms: f64,
    pub longest_ms: f64,
    /// An outage was still open when the run ended.
    pub ongoing: bool,
    /// More than [`MAX_OUTAGE_WINDOWS`] outages happened; only the first
    /// [`MAX_OUTAGE_WINDOWS`] are listed.
    pub windows_truncated: bool,
    pub windows: Vec<OutageWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencySummary {
    pub min_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub max_ms: f64,
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Every `-n` attempt was made.
    Completed,
    /// `--duration` elapsed.
    Duration,
    /// SIGINT (Ctrl-C).
    Interrupt,
    /// SIGTERM.
    Terminated,
    /// The machine ran out of local ports.
    PortsExhausted,
}

impl StopReason {
    /// Short, stable name used in reports and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Completed => "completed",
            StopReason::Duration => "duration",
            StopReason::Interrupt => "interrupt",
            StopReason::Terminated => "terminated",
            StopReason::PortsExhausted => "ports_exhausted",
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Final report of a run.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// Always `"summary"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub protocol: &'static str,
    pub target: String,
    pub concurrency: usize,
    pub conn_mode: &'static str,
    /// Requested number of attempts (`None` = continuous).
    pub requests: Option<u64>,
    pub elapsed_s: f64,
    /// Attempts recorded (including connect failures).
    pub total: u64,
    pub ok: u64,
    /// All failed attempts, including rate-limited ones and mismatches.
    pub errors: u64,
    /// Every error kind, including zero counts.
    pub errors_by_kind: BTreeMap<&'static str, u64>,
    /// `errors / total * 100`.
    pub error_rate_pct: f64,
    /// Attempts per second.
    pub req_per_sec: f64,
    /// Successful requests per second.
    pub ok_per_sec: f64,
    /// Latency of successful requests.
    pub latency: Option<LatencySummary>,
    pub outages: OutageSummary,
    /// Stopped (a signal or `--duration`) before `-n` attempts completed.
    pub interrupted: bool,
    /// Why the run stopped (`completed`, `duration`, `interrupt`,
    /// `terminated` or `ports_exhausted` in JSON).
    pub stop_reason: StopReason,
}

impl Summary {
    pub fn mismatches(&self) -> u64 {
        self.errors_by_kind
            .get(ErrorKind::Mismatch.as_str())
            .copied()
            .unwrap_or(0)
    }
}

/// Stats for one live interval.
#[derive(Debug, Clone)]
pub struct IntervalReport {
    /// Seconds since the start of the run.
    pub elapsed_s: f64,
    /// Actual length of the interval.
    pub length: Duration,
    pub window: Window,
    /// An outage is still open at the end of the interval. Outages that
    /// started and ended within the interval only show up in the error
    /// counts and the outage start/end events.
    pub outage_open: bool,
}

impl IntervalReport {
    pub fn rate(&self) -> f64 {
        per_sec(self.window.count, self.length)
    }
}

fn per_sec(n: u64, d: Duration) -> f64 {
    let secs = d.as_secs_f64();
    if secs > 0.0 { n as f64 / secs } else { 0.0 }
}

/// Something the aggregator reports while running.
#[derive(Debug, Clone)]
pub enum LiveEvent {
    Interval(IntervalReport),
    Outage(OutageEvent),
}

/// Consumes samples from workers and builds interval reports and the summary.
pub struct Aggregator {
    start: Instant,
    total: Window,
    interval: Window,
    interval_start: Instant,
    outages: OutageTracker,
}

impl Aggregator {
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            total: Window::default(),
            interval: Window::default(),
            interval_start: start,
            outages: OutageTracker::new(start),
        }
    }

    fn record(&mut self, sample: &Sample, on_event: &mut impl FnMut(LiveEvent)) {
        // Samples go to the interval window, which is merged into the run
        // totals at every tick and at the end.
        self.interval.record(sample);
        let started = sample.at.checked_sub(sample.latency).unwrap_or(sample.at);
        if let Some(event) = self.outages.observe(started, sample.at, sample.outcome) {
            on_event(LiveEvent::Outage(event));
        }
    }

    fn tick(&mut self, now: Instant) -> IntervalReport {
        let window = std::mem::take(&mut self.interval);
        self.total.merge(&window);
        let report = IntervalReport {
            elapsed_s: now.saturating_duration_since(self.start).as_secs_f64(),
            length: now.saturating_duration_since(self.interval_start),
            window,
            outage_open: self.outages.in_outage(),
        };
        self.interval_start = now;
        report
    }

    /// Runs until every sender is dropped, emitting live events, and returns
    /// the summary. Run metadata (protocol, target, ...) and `interrupted` /
    /// `stop_reason` are left for the caller to fill in.
    pub async fn run(
        mut self,
        mut rx: mpsc::Receiver<Sample>,
        interval: Option<Duration>,
        mut on_event: impl FnMut(LiveEvent),
    ) -> Summary {
        // `interval_at` skips the immediate first tick of `interval`.
        let period = interval.unwrap_or(Duration::from_secs(3600));
        let mut ticker = tokio::time::interval_at(self.start + period, period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                sample = rx.recv() => match sample {
                    Some(sample) => self.record(&sample, &mut on_event),
                    None => break,
                },
                _ = ticker.tick(), if interval.is_some() => {
                    let report = self.tick(Instant::now());
                    on_event(LiveEvent::Interval(report));
                }
            }
        }
        self.finish(Instant::now())
    }

    pub fn finish(mut self, end: Instant) -> Summary {
        let elapsed = end.saturating_duration_since(self.start);
        self.total.merge(&self.interval);
        let total = self.total;
        let errors = total.error_count();
        Summary {
            kind: "summary",
            protocol: "",
            target: String::new(),
            concurrency: 0,
            conn_mode: "",
            requests: None,
            elapsed_s: elapsed.as_secs_f64(),
            total: total.count,
            ok: total.ok,
            errors,
            errors_by_kind: ErrorKind::ALL
                .iter()
                .map(|k| (k.as_str(), total.errors_of(*k)))
                .collect(),
            error_rate_pct: if total.count == 0 {
                0.0
            } else {
                errors as f64 * 100.0 / total.count as f64
            },
            req_per_sec: per_sec(total.count, elapsed),
            ok_per_sec: per_sec(total.ok, elapsed),
            latency: total.latency_summary(),
            outages: self.outages.finish(end),
            interrupted: false,
            stop_reason: StopReason::Completed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_err(kind: io::ErrorKind) -> io::Error {
        io::Error::from(kind)
    }

    #[test]
    fn stop_reason_names_match_serde() {
        for reason in [
            StopReason::Completed,
            StopReason::Duration,
            StopReason::Interrupt,
            StopReason::Terminated,
            StopReason::PortsExhausted,
        ] {
            let json = serde_json::to_value(reason).unwrap();
            assert_eq!(json, reason.as_str());
            assert_eq!(reason.to_string(), reason.as_str());
        }
    }

    #[test]
    fn classify_every_kind() {
        use io::ErrorKind as K;
        let http = |status| EchoError::HttpStatus {
            status,
            reason: String::new(),
            retry_after: None,
            body: String::new(),
        };
        assert_eq!(classify(Phase::Echo, &http(429)), ErrorKind::RateLimited);
        assert_eq!(classify(Phase::Echo, &http(500)), ErrorKind::Other);
        assert_eq!(
            classify(Phase::Echo, &EchoError::Http("garbage".into())),
            ErrorKind::Other
        );
        assert_eq!(
            classify(
                Phase::Connect,
                &EchoError::Tcp(io_err(K::ConnectionRefused))
            ),
            ErrorKind::ConnectRefused
        );
        // Lazy clients (HTTP) surface refusals during the echo phase.
        assert_eq!(
            classify(Phase::Echo, &EchoError::Tcp(io_err(K::ConnectionRefused))),
            ErrorKind::ConnectRefused
        );
        for phase in [Phase::Connect, Phase::Echo] {
            assert_eq!(
                classify(phase, &EchoError::Tcp(io_err(K::AddrNotAvailable))),
                ErrorKind::PortsExhausted
            );
        }
        assert!(!ErrorKind::PortsExhausted.is_outage());
        for kind in [
            K::ConnectionReset,
            K::ConnectionAborted,
            K::BrokenPipe,
            K::UnexpectedEof,
            K::NotConnected,
        ] {
            assert_eq!(
                classify(Phase::Echo, &EchoError::Unix(io_err(kind))),
                ErrorKind::Reset
            );
        }
        assert_eq!(
            classify(Phase::Echo, &EchoError::Timeout("read".into())),
            ErrorKind::Timeout
        );
        assert_eq!(
            classify(Phase::Connect, &EchoError::Udp(io_err(K::TimedOut))),
            ErrorKind::Timeout
        );
        assert_eq!(
            classify(Phase::Connect, &EchoError::Unix(io_err(K::NotFound))),
            ErrorKind::ConnectFailed
        );
        assert_eq!(
            classify(Phase::Connect, &EchoError::Config("x".into())),
            ErrorKind::ConnectFailed
        );
        // A datagram client finds a missing or stale socket when it sends.
        assert_eq!(
            classify(Phase::Echo, &EchoError::Unix(io_err(K::NotFound))),
            ErrorKind::ConnectFailed
        );
        assert_eq!(
            classify(Phase::Echo, &EchoError::Unix(io_err(K::ConnectionRefused))),
            ErrorKind::ConnectRefused
        );
        // A reply over the client's limit is neither an outage cause nor a
        // mismatch the client could check.
        assert_eq!(
            classify(Phase::Echo, &EchoError::Config("too large".into())),
            ErrorKind::Other
        );
        assert_eq!(
            classify(Phase::Echo, &EchoError::Udp(io_err(K::PermissionDenied))),
            ErrorKind::Other
        );
    }

    #[test]
    fn success_older_than_the_outage_does_not_close_it() {
        let t0 = Instant::now();
        let mut t = OutageTracker::new(t0);
        assert!(
            t.observe(t0 + ms(100), t0 + ms(100), Outcome::Err(ErrorKind::Reset))
                .is_some()
        );
        // Reported after the failure, but completed before it.
        assert_eq!(t.observe(t0 + ms(90), t0 + ms(90), Outcome::Ok), None);
        assert!(t.in_outage());
        assert!(matches!(
            t.observe(t0 + ms(300), t0 + ms(300), Outcome::Ok),
            Some(OutageEvent::Ended { .. })
        ));
        let s = t.finish(t0 + ms(400));
        assert_eq!(s.count, 1);
        assert!((s.longest_ms - 200.0).abs() < 1e-6);
    }

    #[test]
    fn in_flight_success_does_not_split_an_outage() {
        let t0 = Instant::now();
        let mut t = OutageTracker::new(t0);
        t.observe(t0 + ms(100), t0 + ms(100), Outcome::Err(ErrorKind::Reset));
        // Started before the first failure, completed just after it: another
        // connection's request that was already in flight.
        assert_eq!(t.observe(t0 + ms(99), t0 + ms(101), Outcome::Ok), None);
        t.observe(
            t0 + ms(150),
            t0 + ms(150),
            Outcome::Err(ErrorKind::ConnectRefused),
        );
        assert!(matches!(
            t.observe(t0 + ms(400), t0 + ms(401), Outcome::Ok),
            Some(OutageEvent::Ended { .. })
        ));
        let s = t.finish(t0 + ms(500));
        assert_eq!(s.count, 1);
        assert!((s.longest_ms - 301.0).abs() < 1e-6, "{s:?}");
    }

    #[test]
    fn error_kind_indices_match_all() {
        for (i, k) in ErrorKind::ALL.iter().enumerate() {
            assert_eq!(k.index(), i);
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn outage_tracker_sequences() {
        let t0 = Instant::now();
        let mut t = OutageTracker::new(t0);
        assert_eq!(t.observe(t0 + ms(10), t0 + ms(10), Outcome::Ok), None);
        assert!(matches!(
            t.observe(
                t0 + ms(100),
                t0 + ms(100),
                Outcome::Err(ErrorKind::ConnectRefused)
            ),
            Some(OutageEvent::Started {
                kind: ErrorKind::ConnectRefused,
                ..
            })
        ));
        assert!(t.in_outage());
        assert_eq!(
            t.observe(t0 + ms(200), t0 + ms(200), Outcome::Err(ErrorKind::Reset)),
            None
        );
        let ended = t.observe(t0 + ms(400), t0 + ms(400), Outcome::Ok);
        match ended {
            Some(OutageEvent::Ended { window, .. }) => {
                assert!((window.duration_ms - 300.0).abs() < 1e-6);
                assert_eq!(window.errors, 2);
                assert!(!window.ongoing);
                assert!((window.start_s - 0.1).abs() < 1e-9);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!t.in_outage());

        // Second, shorter outage that is still open at the end.
        t.observe(t0 + ms(500), t0 + ms(500), Outcome::Err(ErrorKind::Timeout));
        t.observe(t0 + ms(550), t0 + ms(550), Outcome::Ok);
        t.observe(
            t0 + ms(600),
            t0 + ms(600),
            Outcome::Err(ErrorKind::ConnectFailed),
        );
        let s = t.finish(t0 + ms(1000));
        assert_eq!(s.count, 3);
        assert!(s.ongoing);
        assert!((s.longest_ms - 400.0).abs() < 1e-6);
        assert!((s.total_ms - 750.0).abs() < 1e-6);
        assert_eq!(s.windows.len(), 3);
        assert!(s.windows[2].ongoing);
        assert!(!s.windows_truncated);
    }

    #[test]
    fn outage_kinds() {
        let outages: Vec<_> = ErrorKind::ALL
            .into_iter()
            .filter(|k| k.is_outage())
            .collect();
        assert_eq!(
            outages,
            [
                ErrorKind::ConnectRefused,
                ErrorKind::ConnectFailed,
                ErrorKind::Reset,
                ErrorKind::Timeout,
            ]
        );
    }

    #[test]
    fn non_outage_errors_neither_open_nor_extend_an_outage() {
        for kind in [
            ErrorKind::RateLimited,
            ErrorKind::Mismatch,
            ErrorKind::PortsExhausted,
            ErrorKind::Other,
        ] {
            let t0 = Instant::now();
            let mut t = OutageTracker::new(t0);
            for i in 0..10 {
                assert_eq!(
                    t.observe(t0 + ms(i), t0 + ms(i), Outcome::Err(kind)),
                    None,
                    "{kind:?}"
                );
            }
            assert!(!t.in_outage(), "{kind:?}");
            // Nor does it extend an outage that is already open.
            t.observe(t0 + ms(20), t0 + ms(20), Outcome::Err(ErrorKind::Reset));
            t.observe(t0 + ms(30), t0 + ms(30), Outcome::Err(kind));
            t.observe(t0 + ms(40), t0 + ms(40), Outcome::Ok);
            let s = t.finish(t0 + ms(100));
            assert_eq!(s.count, 1, "{kind:?}");
            assert_eq!(s.windows[0].errors, 1, "{kind:?}");
            assert!(!s.ongoing, "{kind:?}");
        }
    }

    #[test]
    fn outage_windows_are_capped() {
        let t0 = Instant::now();
        let mut t = OutageTracker::new(t0);
        for i in 0..150 {
            t.observe(
                t0 + ms(2 * i),
                t0 + ms(2 * i),
                Outcome::Err(ErrorKind::Reset),
            );
            t.observe(t0 + ms(2 * i + 1), t0 + ms(2 * i + 1), Outcome::Ok);
        }
        let s = t.finish(t0 + ms(1000));
        assert_eq!(s.count, 150);
        assert_eq!(s.windows.len(), MAX_OUTAGE_WINDOWS);
        assert!(s.windows_truncated);
    }

    fn sample(latency_us: u64, outcome: Outcome) -> Sample {
        Sample {
            at: Instant::now(),
            latency: Duration::from_micros(latency_us),
            outcome,
        }
    }

    #[test]
    fn window_merge_and_percentiles() {
        let mut a = Window::default();
        let mut b = Window::default();
        for us in 1..=50 {
            a.record(&sample(us * 1000, Outcome::Ok));
        }
        for us in 51..=100 {
            b.record(&sample(us * 1000, Outcome::Ok));
        }
        b.record(&sample(5, Outcome::Err(ErrorKind::Reset)));
        b.record(&sample(5, Outcome::Err(ErrorKind::RateLimited)));
        // Error latencies stay out of the histogram.
        assert_eq!(b.hist.len(), 50);

        a.merge(&b);
        assert_eq!(a.count, 102);
        assert_eq!(a.ok, 100);
        assert_eq!(a.error_count(), 2);
        assert_eq!(a.errors_of(ErrorKind::Reset), 1);
        assert_eq!(
            a.nonzero_errors().into_iter().collect::<Vec<_>>(),
            vec![("rate_limited", 1), ("reset", 1)]
        );

        let p50 = a.percentile(0.5).unwrap().as_secs_f64() * 1000.0;
        let p99 = a.percentile(0.99).unwrap().as_secs_f64() * 1000.0;
        assert!((p50 - 50.0).abs() < 0.1, "p50 = {p50}");
        assert!((p99 - 99.0).abs() < 0.1, "p99 = {p99}");
        assert!(Window::default().percentile(0.5).is_none());
    }

    #[test]
    fn window_clamps_extreme_latencies() {
        let mut w = Window::default();
        w.record(&sample(0, Outcome::Ok));
        w.record(&sample(MAX_LATENCY_US * 10, Outcome::Ok));
        assert_eq!(w.hist.len(), 2);
        assert_eq!(w.hist.min(), 1);
    }

    #[tokio::test]
    async fn aggregator_summary() {
        let start = Instant::now();
        let (tx, rx) = mpsc::channel(16);
        for i in 0..8 {
            tx.send(sample(1000 + i, Outcome::Ok)).await.unwrap();
        }
        tx.send(sample(10, Outcome::Err(ErrorKind::RateLimited)))
            .await
            .unwrap();
        tx.send(sample(10, Outcome::Err(ErrorKind::Mismatch)))
            .await
            .unwrap();
        drop(tx);
        let mut events = Vec::new();
        let summary = Aggregator::new(start)
            .run(rx, Some(Duration::from_secs(60)), |e| events.push(e))
            .await;
        assert_eq!(summary.total, 10);
        assert_eq!(summary.ok, 8);
        assert_eq!(summary.errors, 2);
        assert_eq!(summary.mismatches(), 1);
        assert!((summary.error_rate_pct - 20.0).abs() < 1e-9);
        assert_eq!(summary.errors_by_kind.len(), ErrorKind::COUNT);
        assert_eq!(summary.outages.count, 0); // neither kind is an outage
        assert!(summary.latency.is_some());
        // No interval elapsed, and the first tick is not immediate.
        assert!(events.iter().all(|e| !matches!(e, LiveEvent::Interval(_))));
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["type"], "summary");
        assert_eq!(json["errors_by_kind"]["rate_limited"], 1);
    }

    #[tokio::test(start_paused = true)]
    async fn aggregator_emits_intervals() {
        let start = Instant::now();
        let (tx, rx) = mpsc::channel(16);
        let producer = tokio::spawn(async move {
            for _ in 0..3 {
                tx.send(sample(100, Outcome::Ok)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        });
        let mut intervals = Vec::new();
        let summary = Aggregator::new(start)
            .run(rx, Some(Duration::from_millis(1000)), |e| {
                if let LiveEvent::Interval(r) = e {
                    intervals.push(r);
                }
            })
            .await;
        producer.await.unwrap();
        assert_eq!(summary.ok, 3);
        assert!(intervals.len() >= 2, "got {} intervals", intervals.len());
        assert!((intervals[0].elapsed_s - 1.0).abs() < 0.01);
        assert!(intervals.iter().map(|r| r.window.count).sum::<u64>() <= 3);
    }

    fn sample_at(at: Instant, outcome: Outcome) -> Sample {
        Sample {
            at,
            latency: Duration::from_micros(100),
            outcome,
        }
    }

    #[test]
    fn interval_outage_open_means_open_at_end() {
        let t0 = Instant::now();
        let mut agg = Aggregator::new(t0);
        let mut events = Vec::new();
        let mut on_event = |e| events.push(e);
        let refused = Outcome::Err(ErrorKind::ConnectRefused);

        // An outage that starts and ends within the interval: the errors are
        // counted, but no outage is open at the end.
        agg.record(&sample_at(t0 + ms(100), Outcome::Ok), &mut on_event);
        agg.record(&sample_at(t0 + ms(200), refused), &mut on_event);
        agg.record(&sample_at(t0 + ms(300), refused), &mut on_event);
        agg.record(&sample_at(t0 + ms(400), Outcome::Ok), &mut on_event);
        let r = agg.tick(t0 + ms(1000));
        assert!(!r.outage_open);
        assert_eq!(r.window.errors_of(ErrorKind::ConnectRefused), 2);

        // Starts mid-interval and is still open at the end.
        agg.record(&sample_at(t0 + ms(1100), Outcome::Ok), &mut on_event);
        agg.record(&sample_at(t0 + ms(1500), refused), &mut on_event);
        let r = agg.tick(t0 + ms(2000));
        assert!(r.outage_open);
        assert_eq!(r.window.ok, 1);

        // Open across a whole interval, with and without new samples.
        agg.record(&sample_at(t0 + ms(2500), refused), &mut on_event);
        assert!(agg.tick(t0 + ms(3000)).outage_open);
        let r = agg.tick(t0 + ms(4000));
        assert!(r.outage_open);
        assert_eq!(r.window.count, 0);

        // Ends mid-interval: no marker, even though it was open at the start.
        agg.record(&sample_at(t0 + ms(4200), Outcome::Ok), &mut on_event);
        assert!(!agg.tick(t0 + ms(5000)).outage_open);

        let kinds: Vec<_> = events
            .iter()
            .map(|e| match e {
                LiveEvent::Outage(OutageEvent::Started { .. }) => "start",
                LiveEvent::Outage(OutageEvent::Ended { .. }) => "end",
                LiveEvent::Interval(_) => "interval",
            })
            .collect();
        assert_eq!(kinds, ["start", "end", "start", "end"]);
    }
}
