//! Load generation: workers, the shared shaper and the sample pipeline.
//!
//! This is the only module that knows how to build a client for each
//! protocol; everything else works through [`EchoClient`].

use crate::header::RunInfo;
use crate::stats::{
    Aggregator, ErrorKind, LiveEvent, Outcome, Phase, Sample, StopReason, Summary, classify,
};
use clap::ValueEnum;
use echosrv::cli::Protocol;
use echosrv::datagram::DatagramClientConfig;
use echosrv::stream::ClientConfig;
use echosrv::{
    EchoClient, EchoError, HttpEchoClient, RateLimitConfig, TcpEchoClient, TokenBucket,
    UdpEchoClient, UnixDatagramEchoClient, UnixStreamEchoClient,
};
use std::fmt;
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// Default filler pattern after the sequence header.
const PATTERN: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
/// Samples buffered between workers and the aggregator.
const SAMPLE_CHANNEL_CAPACITY: usize = 8192;
/// Read buffer for stream clients.
const STREAM_BUFFER_SIZE: usize = 16 * 1024;
/// Smallest receive buffer for datagram clients (covers any UDP payload).
const DATAGRAM_BUFFER_SIZE: usize = 65_536;
/// Upper bound on the `echosrv:<worker>:<seq>:` header length.
const MAX_HEADER_LEN: usize = 64;
/// Payload size without `--payload-size` or `--payload`, header included.
pub const DEFAULT_PAYLOAD_SIZE: usize = 64;
/// Default cap on new connections per second, across all workers; well
/// under [`SAFE_CONN_RATE`].
pub const DEFAULT_CONN_RATE: u32 = 100;
/// Highest sustained rate of new TCP connections per second that is safe
/// for the machine the client runs on; `--conn-rate` above it warns.
///
/// Every closed TCP connection holds its local port in TIME_WAIT: about 30s
/// on macOS, 60s on Linux. The ephemeral port range has about 16k ports on
/// macOS and 28k on Linux, so the machine runs out of local ports at roughly
/// 16k / 30s = 530/s (macOS) or 28k / 60s = 470/s (Linux), and networking
/// then stalls for every application on it, not just the load test. This
/// limit leaves a margin below both. Other code and messages that mention
/// the port-exhaustion threshold refer here.
pub const SAFE_CONN_RATE: u32 = 400;

/// Where and how to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    Tcp(SocketAddr),
    Udp(SocketAddr),
    Http(SocketAddr),
    UnixStream(PathBuf),
    UnixDgram(PathBuf),
}

impl Transport {
    pub fn protocol(&self) -> Protocol {
        match self {
            Transport::Tcp(_) => Protocol::Tcp,
            Transport::Udp(_) => Protocol::Udp,
            Transport::Http(_) => Protocol::Http,
            Transport::UnixStream(_) => Protocol::UnixStream,
            Transport::UnixDgram(_) => Protocol::UnixDatagram,
        }
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transport::Tcp(a) | Transport::Udp(a) | Transport::Http(a) => write!(f, "{a}"),
            Transport::UnixStream(p) | Transport::UnixDgram(p) => write!(f, "{}", p.display()),
        }
    }
}

/// Connection reuse policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConnMode {
    /// Keep one connection (or socket) per worker; reconnect after errors.
    Persistent,
    /// Open a new connection (or socket) for every request.
    PerRequest,
}

impl ConnMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnMode::Persistent => "persistent",
            ConnMode::PerRequest => "per-request",
        }
    }
}

/// What follows the `echosrv:<worker>:<seq>:` header in each payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filler {
    /// A fixed alphanumeric pattern.
    Pattern,
    /// User text, repeated or truncated to the payload size.
    Text(Vec<u8>),
    /// Pseudo-random bytes.
    Random,
}

impl Filler {
    pub fn as_str(&self) -> &'static str {
        match self {
            Filler::Pattern => "pattern",
            Filler::Text(_) => "text",
            Filler::Random => "random",
        }
    }
}

/// Everything a run needs, already validated.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub transport: Transport,
    /// Total attempts; `None` runs until stopped.
    pub requests: Option<u64>,
    /// Stop after this long (`--duration`); `None` = no time limit.
    pub duration: Option<Duration>,
    pub concurrency: usize,
    /// Client-side shaping (token bucket).
    pub rate: Option<RateLimitConfig>,
    /// Payload size (grown to fit the header). `None` = header + text.
    pub payload_size: Option<usize>,
    pub filler: Filler,
    pub timeout: Duration,
    pub conn_mode: ConnMode,
    /// Cap on new connections (client creations) per second across all
    /// workers; `None` = unlimited.
    pub conn_rate: Option<RateLimitConfig>,
    /// Pause after the first error in a row; it doubles with each further
    /// consecutive error, up to `max_backoff`.
    pub reconnect_delay: Duration,
    pub max_backoff: Duration,
    pub honor_retry_after: bool,
    /// Live stats interval; `None` disables interval events.
    pub interval: Option<Duration>,
}

impl RunConfig {
    /// An upper bound on the size of any payload of this run, used to size
    /// the clients' response limits so a correct echo always fits.
    pub fn max_payload_len(&self) -> usize {
        match (self.payload_size, &self.filler) {
            (Some(size), _) => size.max(MAX_HEADER_LEN),
            (None, Filler::Text(text)) => MAX_HEADER_LEN + text.len(),
            (None, _) => MAX_HEADER_LEN,
        }
    }

    /// A config with the CLI defaults for `transport`.
    #[cfg(test)]
    pub fn new(transport: Transport) -> Self {
        let conn_mode = if matches!(transport, Transport::Http(_)) {
            ConnMode::PerRequest
        } else {
            ConnMode::Persistent
        };
        Self {
            transport,
            requests: None,
            duration: None,
            concurrency: 1,
            rate: None,
            payload_size: Some(DEFAULT_PAYLOAD_SIZE),
            filler: Filler::Pattern,
            timeout: Duration::from_secs(5),
            conn_mode,
            conn_rate: Some(RateLimitConfig::new(DEFAULT_CONN_RATE, DEFAULT_CONN_RATE)),
            reconnect_delay: Duration::from_millis(100),
            max_backoff: Duration::from_millis(200),
            honor_retry_after: false,
            interval: None,
        }
    }
}

/// Builds a client for `transport`; stream clients (including HTTP) connect
/// here. Every connect, read and write is bounded by `timeout`, and the
/// response limits fit payloads of up to `max_payload` bytes.
pub async fn make_client(
    transport: &Transport,
    timeout: Duration,
    max_payload: usize,
) -> echosrv::Result<Box<dyn EchoClient + Send>> {
    let stream_config = || ClientConfig {
        read_timeout: timeout,
        write_timeout: timeout,
        connect_timeout: timeout,
        buffer_size: STREAM_BUFFER_SIZE,
        max_response_size: ClientConfig::default().max_response_size.max(max_payload),
    };
    let datagram_config = || DatagramClientConfig {
        buffer_size: DATAGRAM_BUFFER_SIZE.max(max_payload),
        read_timeout: timeout,
        write_timeout: timeout,
    };
    let connect = async {
        let client: Box<dyn EchoClient + Send> = match transport {
            Transport::Tcp(addr) => {
                Box::new(TcpEchoClient::connect_with_config(*addr, stream_config()).await?)
            }
            Transport::Http(addr) => {
                Box::new(HttpEchoClient::connect_with_config(*addr, stream_config()).await?)
            }
            Transport::Udp(addr) => {
                Box::new(UdpEchoClient::connect_with_config(*addr, datagram_config()).await?)
            }
            Transport::UnixStream(path) => Box::new(
                UnixStreamEchoClient::connect_with_config(path.clone(), stream_config()).await?,
            ),
            Transport::UnixDgram(path) => Box::new(
                UnixDatagramEchoClient::connect_with_config(path.clone(), datagram_config())
                    .await?,
            ),
        };
        Ok(client)
    };
    tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| EchoError::Timeout("connect timeout".into()))?
}

/// Tiny xorshift64* generator for random filler (no `rand` dependency).
#[derive(Debug, Clone)]
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        // Mix the seed (splitmix64) so nearby seeds diverge; never zero.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self((z ^ (z >> 31)) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

/// Writes the payload for (`worker`, `seq`) into `buf`: the unique header
/// `echosrv:<worker>:<seq>:` followed by filler up to `size` bytes (never
/// shorter than the header). With `size == None` the filler is appended once
/// (the `--payload TEXT` default).
pub fn build_payload(
    buf: &mut Vec<u8>,
    worker: usize,
    seq: u64,
    size: Option<usize>,
    filler: &Filler,
    rng: &mut XorShift,
) {
    use std::io::Write;
    buf.clear();
    write!(buf, "echosrv:{worker}:{seq}:").expect("writing to a Vec cannot fail");
    let header = buf.len();
    let total = match (size, filler) {
        (Some(size), _) => size.max(header),
        (None, Filler::Text(text)) => header + text.len(),
        (None, _) => header.max(DEFAULT_PAYLOAD_SIZE),
    };
    match filler {
        Filler::Pattern => buf.extend(PATTERN.iter().cycle().take(total - header)),
        Filler::Text(text) => buf.extend(text.iter().cycle().take(total - header)),
        Filler::Random => {
            buf.resize(total, 0);
            rng.fill(&mut buf[header..]);
        }
    }
}

/// Takes the next sequence number, or `None` once `limit` is reached.
fn next_seq(counter: &AtomicU64, limit: Option<u64>) -> Option<u64> {
    match limit {
        None => Some(counter.fetch_add(1, Ordering::Relaxed)),
        Some(limit) => counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
                (s < limit).then_some(s + 1)
            })
            .ok(),
    }
}

/// Exponential backoff over a worker's consecutive errors. Every error drops
/// the client, so without a pause a failing or rejecting server would be hit
/// by a tight reconnect loop.
#[derive(Debug, Clone, Copy)]
struct Backoff {
    /// Consecutive errors so far.
    errors: u32,
    /// Pause after the first error.
    base: Duration,
    /// Cap on the pause.
    max: Duration,
}

impl Backoff {
    fn new(base: Duration, max: Duration) -> Self {
        Self {
            errors: 0,
            base,
            max,
        }
    }

    /// Records an error and returns the pause before the next attempt:
    /// `base`, doubled for each further consecutive error, capped at `max`.
    fn fail(&mut self) -> Duration {
        self.errors = self.errors.saturating_add(1);
        let doublings = (self.errors - 1).min(16);
        self.base.saturating_mul(1 << doublings).min(self.max)
    }

    /// Records a success: the next error pauses for `base` again.
    fn reset(&mut self) {
        self.errors = 0;
    }
}

/// Sleeps for `d` unless cancelled first. Returns `false` if cancelled.
async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> bool {
    if d.is_zero() {
        return !cancel.is_cancelled();
    }
    tokio::select! {
        () = cancel.cancelled() => false,
        () = tokio::time::sleep(d) => true,
    }
}

/// A connected client of any protocol.
type Client = Box<dyn EchoClient + Send>;

struct Worker {
    id: usize,
    config: Arc<RunConfig>,
    seq: Arc<AtomicU64>,
    shaper: Option<Arc<TokenBucket>>,
    /// Shared cap on new connections.
    conn_limiter: Option<Arc<TokenBucket>>,
    tx: mpsc::Sender<Sample>,
    cancel: CancellationToken,
    /// Why the run stopped; the first writer wins.
    stop_reason: Arc<OnceLock<StopReason>>,
}

impl Worker {
    async fn record(&self, started: Instant, outcome: Outcome) -> bool {
        let at = Instant::now();
        let sample = Sample {
            at,
            latency: at.saturating_duration_since(started),
            outcome,
        };
        self.tx.send(sample).await.is_ok()
    }

    /// Classifies an echo result. Returns the outcome and, for an honored
    /// HTTP 429, the server's Retry-After (which replaces the backoff).
    fn evaluate(
        &self,
        seq: u64,
        payload: &[u8],
        result: echosrv::Result<Vec<u8>>,
    ) -> (Outcome, Option<Duration>) {
        let config = &*self.config;
        match result {
            Ok(reply) if reply == payload => (Outcome::Ok, None),
            Ok(reply) => {
                debug!(
                    worker = self.id,
                    seq,
                    sent = payload.len(),
                    received = reply.len(),
                    "echo mismatch"
                );
                (Outcome::Err(ErrorKind::Mismatch), None)
            }
            Err(e) => {
                let kind = classify(Phase::Echo, &e);
                debug!(worker = self.id, seq, error = %e, ?kind, "echo failed");
                let retry_after = e
                    .retry_after()
                    .filter(|_| config.honor_retry_after && kind == ErrorKind::RateLimited);
                (Outcome::Err(kind), retry_after)
            }
        }
    }

    /// On [`ErrorKind::PortsExhausted`], stops the whole run with
    /// [`StopReason::PortsExhausted`]: retrying only keeps the machine out of
    /// ports. Returns whether the worker should stop.
    fn stop_if_ports_exhausted(&self, kind: ErrorKind) -> bool {
        if kind != ErrorKind::PortsExhausted {
            return false;
        }
        stop(&self.cancel, &self.stop_reason, StopReason::PortsExhausted);
        true
    }

    /// Waits for a token from `bucket`, if there is one. Returns `false` if
    /// the run is cancelled first.
    async fn acquire(&self, bucket: Option<&TokenBucket>) -> bool {
        let Some(bucket) = bucket else {
            return true;
        };
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => false,
            () = bucket.acquire() => true,
        }
    }

    /// Connects a new client under the connection limiter. A failed connect
    /// is recorded and backed off, then `Continue(None)` moves on to the next
    /// attempt; `Break` stops the worker.
    async fn connect(&self, seq: u64, backoff: &mut Backoff) -> ControlFlow<(), Option<Client>> {
        if !self.acquire(self.conn_limiter.as_deref()).await {
            return ControlFlow::Break(());
        }
        let config = &*self.config;
        // Not cancellable: once an attempt has started, a stop lets it
        // finish (bounded by the timeout) so it is recorded and its
        // connection is closed cleanly instead of dropped mid-request.
        let started = Instant::now();
        let result = make_client(&config.transport, config.timeout, config.max_payload_len()).await;
        match result {
            Ok(c) => ControlFlow::Continue(Some(c)),
            Err(e) => {
                let kind = classify(Phase::Connect, &e);
                debug!(worker = self.id, seq, error = %e, ?kind, "connect failed");
                if !self.record(started, Outcome::Err(kind)).await
                    || self.stop_if_ports_exhausted(kind)
                    || !sleep_or_cancel(backoff.fail(), &self.cancel).await
                {
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(None)
            }
        }
    }

    /// Sends `payload` (request `seq`) over the connected `client` and
    /// records the result. An error drops the client, as does per-request
    /// mode. Returns the pause before the next attempt, or `Break` to stop
    /// the worker.
    async fn echo(
        &self,
        client: &mut Option<Client>,
        seq: u64,
        payload: &[u8],
        backoff: &mut Backoff,
    ) -> ControlFlow<(), Duration> {
        let Some(c) = client.as_mut() else {
            unreachable!("echo needs a connected client")
        };
        // Runs to completion even if the run is stopped meanwhile (see
        // `connect`); every read and write is bounded by the timeout.
        let started = Instant::now();
        let result = c.echo(payload).await;

        let (outcome, retry_after) = self.evaluate(seq, payload, result);
        let pause = match outcome {
            Outcome::Ok => {
                backoff.reset();
                Duration::ZERO
            }
            Outcome::Err(_) => {
                *client = None;
                let pause = backoff.fail();
                retry_after.unwrap_or(pause)
            }
        };
        if !self.record(started, outcome).await {
            return ControlFlow::Break(());
        }
        if let Outcome::Err(kind) = outcome {
            if self.stop_if_ports_exhausted(kind) {
                return ControlFlow::Break(());
            }
        }
        if self.config.conn_mode == ConnMode::PerRequest {
            *client = None;
        }
        ControlFlow::Continue(pause)
    }

    async fn run(self) {
        let config = &*self.config;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() ^ (u64::from(d.subsec_nanos()) << 32));
        let mut rng = XorShift::new(nanos ^ (self.id as u64).rotate_left(48));
        let mut payload = Vec::new();
        let mut client: Option<Client> = None;
        let mut backoff = Backoff::new(config.reconnect_delay, config.max_backoff);

        while !self.cancel.is_cancelled() {
            let Some(seq) = next_seq(&self.seq, config.requests) else {
                break;
            };
            if !self.acquire(self.shaper.as_deref()).await {
                break;
            }
            if client.is_none() {
                match self.connect(seq, &mut backoff).await {
                    ControlFlow::Break(()) => break,
                    ControlFlow::Continue(None) => continue,
                    ControlFlow::Continue(Some(c)) => client = Some(c),
                }
            }
            build_payload(
                &mut payload,
                self.id,
                seq,
                config.payload_size,
                &config.filler,
                &mut rng,
            );
            let ControlFlow::Continue(pause) =
                self.echo(&mut client, seq, &payload, &mut backoff).await
            else {
                break;
            };
            if !sleep_or_cancel(pause, &self.cancel).await {
                break;
            }
        }
    }
}

/// Records `reason` (unless a reason is already set) and cancels the run.
pub fn stop(cancel: &CancellationToken, stop_reason: &OnceLock<StopReason>, reason: StopReason) {
    let _ = stop_reason.set(reason);
    cancel.cancel();
}

/// Runs the load test until `config.requests` attempts are done, until
/// `config.duration` elapses or until `cancel` fires, calling `on_event` for
/// live intervals and outage changes.
///
/// Cancelling is a graceful stop: no new attempts start, while attempts
/// already in flight finish (each bounded by `config.timeout`) and are
/// recorded. To abort in-flight requests, drop the future (or exit).
///
/// `stop_reason` is shared with whoever else may stop the run (a signal
/// handler): set it before cancelling, ideally through [`stop`]. The first
/// reason set wins. The run itself sets [`StopReason::Duration`] and
/// [`StopReason::PortsExhausted`], and [`StopReason::Completed`] if nothing
/// else stopped it first.
///
/// The returned summary is complete: run metadata, statistics and the stop
/// reason.
pub async fn run(
    config: RunConfig,
    cancel: CancellationToken,
    stop_reason: Arc<OnceLock<StopReason>>,
    on_event: impl FnMut(LiveEvent),
) -> Summary {
    let config = Arc::new(config);
    let start = Instant::now();
    let seq = Arc::new(AtomicU64::new(0));
    let shaper = config.rate.map(|r| Arc::new(TokenBucket::new(r)));
    let conn_limiter = config.conn_rate.map(|r| Arc::new(TokenBucket::new(r)));
    let (tx, rx) = mpsc::channel(SAMPLE_CHANNEL_CAPACITY);

    let timer = config.duration.map(|duration| {
        let cancel = cancel.clone();
        let stop_reason = stop_reason.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(duration) => {
                    stop(&cancel, &stop_reason, StopReason::Duration);
                }
            }
        })
    });

    let workers: Vec<_> = (0..config.concurrency.max(1))
        .map(|id| {
            let worker = Worker {
                id,
                config: config.clone(),
                seq: seq.clone(),
                shaper: shaper.clone(),
                conn_limiter: conn_limiter.clone(),
                tx: tx.clone(),
                cancel: cancel.clone(),
                stop_reason: stop_reason.clone(),
            };
            tokio::spawn(worker.run())
        })
        .collect();
    drop(tx);

    let stats = Aggregator::new(start)
        .run(rx, config.interval, on_event)
        .await;
    // Every worker has dropped its sender, so the run is over: settle the
    // reason before the timer is gone, so a late timer cannot relabel a run
    // that completed.
    let reason = *stop_reason.get_or_init(|| StopReason::Completed);
    if let Some(timer) = timer {
        timer.abort();
    }
    for worker in workers {
        if let Err(e) = worker.await {
            tracing::error!(error = %e, "worker task failed");
        }
    }

    Summary::new(RunInfo::new(&config), stats, reason)
}

#[cfg(test)]
mod tests;
