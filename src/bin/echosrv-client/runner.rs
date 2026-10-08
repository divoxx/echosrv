//! Load generation: workers, the shared shaper and the sample pipeline.
//!
//! This is the only module that knows how to build a client for each
//! protocol; everything else works through [`EchoClient`].

use crate::stats::{Aggregator, ErrorKind, LiveEvent, Outcome, Phase, Sample, Summary, classify};
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
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// Default cap on new connections per second, across all workers.
///
/// Every closed TCP connection holds a local port in TIME_WAIT for 30-60s,
/// and the ephemeral range has ~16k ports (macOS) to ~28k (Linux), so a
/// sustained rate above roughly 500/s runs the machine out of ports and
/// stalls networking for every application on it.
pub const DEFAULT_CONN_RATE: u32 = 100;
/// `stop_reason` of a run stopped because the machine ran out of ports.
pub const STOP_PORTS_EXHAUSTED: &str = "ports_exhausted";

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
    /// Total attempts; `None` runs until cancelled.
    pub requests: Option<u64>,
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
            concurrency: 1,
            rate: None,
            payload_size: Some(64),
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
        (None, _) => header.max(64),
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

/// Pause after `errors` consecutive errors (at least 1): `base`, doubled for
/// each further error, capped at `max`.
fn backoff(base: Duration, max: Duration, errors: u32) -> Duration {
    let doublings = errors.saturating_sub(1).min(16);
    base.saturating_mul(1 << doublings).min(max)
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

struct Worker {
    id: usize,
    config: Arc<RunConfig>,
    seq: Arc<AtomicU64>,
    shaper: Option<Arc<TokenBucket>>,
    /// Shared cap on new connections.
    conn_limiter: Option<Arc<TokenBucket>>,
    tx: mpsc::Sender<Sample>,
    cancel: CancellationToken,
    /// Set (and the run cancelled) when the machine runs out of ports.
    ports_exhausted: Arc<AtomicBool>,
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

    /// On [`ErrorKind::PortsExhausted`], flags it and cancels the whole run:
    /// retrying only keeps the machine out of ports. Returns whether the
    /// worker should stop.
    fn stop_if_ports_exhausted(&self, kind: ErrorKind) -> bool {
        if kind != ErrorKind::PortsExhausted {
            return false;
        }
        self.ports_exhausted.store(true, Ordering::Relaxed);
        self.cancel.cancel();
        true
    }

    async fn run(self) {
        let config = &*self.config;
        let cancel = &self.cancel;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() ^ (u64::from(d.subsec_nanos()) << 32));
        let mut rng = XorShift::new(nanos ^ (self.id as u64).rotate_left(48));
        let mut payload = Vec::new();
        let mut client: Option<Box<dyn EchoClient + Send>> = None;
        let max_payload = config.max_payload_len();
        // Consecutive errors of this worker, for the backoff. Every error
        // drops the client, so without a pause a failing or rejecting server
        // would be hit by a tight reconnect loop.
        let mut errors: u32 = 0;

        while !cancel.is_cancelled() {
            let Some(seq) = next_seq(&self.seq, config.requests) else {
                break;
            };

            if let Some(shaper) = &self.shaper {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    () = shaper.acquire() => {}
                }
            }

            if client.is_none() {
                if let Some(limiter) = &self.conn_limiter {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        () = limiter.acquire() => {}
                    }
                }
                // Not cancellable: once an attempt has started, a stop lets it
                // finish (bounded by the timeout) so it is recorded and its
                // connection is closed cleanly instead of dropped mid-request.
                let started = Instant::now();
                let result = make_client(&config.transport, config.timeout, max_payload).await;
                match result {
                    Ok(c) => client = Some(c),
                    Err(e) => {
                        let kind = classify(Phase::Connect, &e);
                        debug!(worker = self.id, seq, error = %e, ?kind, "connect failed");
                        if !self.record(started, Outcome::Err(kind)).await
                            || self.stop_if_ports_exhausted(kind)
                        {
                            break;
                        }
                        errors = errors.saturating_add(1);
                        let pause = backoff(config.reconnect_delay, config.max_backoff, errors);
                        if !sleep_or_cancel(pause, cancel).await {
                            break;
                        }
                        continue;
                    }
                }
            }
            let Some(c) = client.as_mut() else {
                unreachable!("client was just connected")
            };

            build_payload(
                &mut payload,
                self.id,
                seq,
                config.payload_size,
                &config.filler,
                &mut rng,
            );
            // Runs to completion even if the run is stopped meanwhile (see
            // the connect above); every read and write is bounded by the
            // timeout.
            let started = Instant::now();
            let result = c.echo(&payload).await;

            let (outcome, retry_after) = self.evaluate(seq, &payload, result);
            let pause = match outcome {
                Outcome::Ok => {
                    errors = 0;
                    Duration::ZERO
                }
                Outcome::Err(_) => {
                    client = None;
                    errors = errors.saturating_add(1);
                    retry_after.unwrap_or_else(|| {
                        backoff(config.reconnect_delay, config.max_backoff, errors)
                    })
                }
            };
            if !self.record(started, outcome).await {
                break;
            }
            if let Outcome::Err(kind) = outcome {
                if self.stop_if_ports_exhausted(kind) {
                    break;
                }
            }
            if config.conn_mode == ConnMode::PerRequest {
                client = None;
            }
            if !sleep_or_cancel(pause, cancel).await {
                break;
            }
        }
    }
}

/// Runs the load test until `config.requests` attempts are done or `cancel`
/// fires, calling `on_event` for live intervals and outage changes.
///
/// Cancelling is a graceful stop: no new attempts start, while attempts
/// already in flight finish (each bounded by `config.timeout`) and are
/// recorded. To abort in-flight requests, drop the future (or exit).
///
/// The returned summary has run metadata filled in. `stop_reason` is
/// [`STOP_PORTS_EXHAUSTED`] if the run stopped itself because the machine ran
/// out of ports, and otherwise `completed` for the caller to adjust.
pub async fn run(
    config: RunConfig,
    cancel: CancellationToken,
    on_event: impl FnMut(LiveEvent),
) -> Summary {
    let config = Arc::new(config);
    let start = Instant::now();
    let seq = Arc::new(AtomicU64::new(0));
    let shaper = config.rate.map(|r| Arc::new(TokenBucket::new(r)));
    let conn_limiter = config.conn_rate.map(|r| Arc::new(TokenBucket::new(r)));
    let ports_exhausted = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel(SAMPLE_CHANNEL_CAPACITY);

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
                ports_exhausted: ports_exhausted.clone(),
            };
            tokio::spawn(worker.run())
        })
        .collect();
    drop(tx);

    let mut summary = Aggregator::new(start)
        .run(rx, config.interval, on_event)
        .await;
    for worker in workers {
        if let Err(e) = worker.await {
            tracing::error!(error = %e, "worker task failed");
        }
    }

    summary.protocol = config.transport.protocol().as_str().to_string();
    summary.target = config.transport.to_string();
    summary.concurrency = config.concurrency;
    summary.conn_mode = config.conn_mode.as_str().to_string();
    summary.requests = config.requests;
    summary.interrupted = config.requests.is_some_and(|n| summary.total < n);
    if ports_exhausted.load(Ordering::Relaxed) {
        summary.stop_reason = STOP_PORTS_EXHAUSTED;
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::OutageEvent;
    use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
    use echosrv::{
        EchoServerTrait, HttpConfig, HttpEchoServer, TcpConfig, TcpEchoServer, UdpConfig,
        UdpEchoServer, UnixDatagramEchoServer, UnixStreamEchoServer,
    };
    use std::future::Future;
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::broadcast;
    use tokio::task::JoinHandle;

    /// Upper bound for anything a test waits on.
    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn payload_header_and_size() {
        let mut rng = XorShift::new(1);
        let mut buf = Vec::new();
        build_payload(&mut buf, 3, 42, Some(64), &Filler::Pattern, &mut rng);
        assert_eq!(buf.len(), 64);
        assert!(buf.starts_with(b"echosrv:3:42:abcdef"));

        // Grown to fit the header.
        build_payload(&mut buf, 12, 1_000_000, Some(4), &Filler::Pattern, &mut rng);
        assert_eq!(buf, b"echosrv:12:1000000:");

        // Text is repeated / truncated to the size, or appended once.
        build_payload(
            &mut buf,
            0,
            7,
            Some(20),
            &Filler::Text(b"xy".to_vec()),
            &mut rng,
        );
        assert_eq!(buf, b"echosrv:0:7:xyxyxyxy");
        build_payload(
            &mut buf,
            0,
            7,
            None,
            &Filler::Text(b"hello".to_vec()),
            &mut rng,
        );
        assert_eq!(buf, b"echosrv:0:7:hello");

        // Random filler has the right size and differs between requests.
        build_payload(&mut buf, 1, 1, Some(256), &Filler::Random, &mut rng);
        let first = buf.clone();
        build_payload(&mut buf, 1, 1, Some(256), &Filler::Random, &mut rng);
        assert_eq!(buf.len(), 256);
        assert!(buf.starts_with(b"echosrv:1:1:"));
        assert_ne!(first, buf);
    }

    #[test]
    fn payloads_are_unique() {
        let mut rng = XorShift::new(1);
        let mut seen = std::collections::HashSet::new();
        let mut buf = Vec::new();
        for worker in 0..4 {
            for seq in 0..100 {
                build_payload(&mut buf, worker, seq, Some(32), &Filler::Pattern, &mut rng);
                assert!(seen.insert(buf.clone()));
            }
        }
    }

    #[test]
    fn max_payload_len_bounds_every_payload() {
        let mut rng = XorShift::new(7);
        let mut buf = Vec::new();
        let worst = (9_999, u64::MAX);
        for (size, filler) in [
            (None, Filler::Pattern),
            (None, Filler::Random),
            (None, Filler::Text(b"some text".to_vec())),
            (Some(1), Filler::Pattern),
            (Some(100_000), Filler::Random),
        ] {
            let config = RunConfig {
                payload_size: size,
                filler: filler.clone(),
                ..RunConfig::new(Transport::Tcp("127.0.0.1:1".parse().unwrap()))
            };
            build_payload(&mut buf, worst.0, worst.1, size, &filler, &mut rng);
            assert!(buf.len() <= config.max_payload_len(), "{size:?} {filler:?}");
        }
    }

    #[test]
    fn next_seq_respects_limit() {
        let c = AtomicU64::new(0);
        assert_eq!(next_seq(&c, Some(2)), Some(0));
        assert_eq!(next_seq(&c, Some(2)), Some(1));
        assert_eq!(next_seq(&c, Some(2)), None);
        assert_eq!(next_seq(&c, Some(2)), None);
        assert_eq!(next_seq(&c, None), Some(2));
    }

    // ---- runner tests against in-process servers -------------------------
    //
    // Servers are bound (port 0 or a fresh temp path) before the run starts,
    // so the address is real and already accepting: no port picking, no
    // readiness probes and no serialization between tests.

    /// A server serving in a background task.
    struct Running {
        shutdown: broadcast::Sender<()>,
        handle: JoinHandle<echosrv::Result<()>>,
    }

    impl Running {
        fn spawn(
            shutdown: broadcast::Sender<()>,
            serve: impl Future<Output = echosrv::Result<()>> + Send + 'static,
        ) -> Self {
            Self {
                shutdown,
                handle: tokio::spawn(serve),
            }
        }

        /// Graceful shutdown: the listener is closed and open connections
        /// are cancelled before this returns.
        async fn stop(self) {
            self.shutdown.send(()).unwrap();
            tokio::time::timeout(WAIT, self.handle)
                .await
                .expect("server did not stop")
                .unwrap()
                .unwrap();
        }
    }

    fn localhost() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    async fn tcp_server(bind_addr: SocketAddr) -> (SocketAddr, Running) {
        let server = TcpEchoServer::new(
            TcpConfig {
                bind_addr,
                ..Default::default()
            }
            .into(),
        );
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.unwrap();
        let addr = *bound.local_addr().as_network().unwrap();
        (addr, Running::spawn(shutdown, bound.serve()))
    }

    async fn http_server(rate_limit: Option<RateLimitConfig>) -> (SocketAddr, Running) {
        let server = HttpEchoServer::new(HttpConfig {
            rate_limit,
            ..Default::default()
        });
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.unwrap();
        let addr = *bound.local_addr().as_network().unwrap();
        (addr, Running::spawn(shutdown, bound.serve()))
    }

    async fn udp_server() -> (SocketAddr, Running) {
        let server = UdpEchoServer::new(UdpConfig::default().into());
        let shutdown = server.shutdown_signal();
        let bound = server.bind().await.unwrap();
        let addr = *bound.local_addr().as_network().unwrap();
        (addr, Running::spawn(shutdown, bound.serve()))
    }

    async fn unix_stream_server(path: &Path) -> Running {
        let server =
            UnixStreamEchoServer::new(UnixStreamConfig::default().with_socket_path(path.into()));
        let shutdown = server.shutdown_signal();
        Running::spawn(shutdown, server.bind().await.unwrap().serve())
    }

    async fn unix_dgram_server(path: &Path) -> Running {
        let server = UnixDatagramEchoServer::new(
            UnixDatagramConfig::default().with_socket_path(path.into()),
        );
        let shutdown = server.shutdown_signal();
        Running::spawn(shutdown, server.bind().await.unwrap().serve())
    }

    /// An address nothing listens on. Port 1 is privileged (tests never bind
    /// it) and outside the ephemeral range, so a client can neither race
    /// another test for it nor connect to itself. (A bound but not listening
    /// socket would not do: macOS drops SYNs to it instead of refusing.)
    fn refused_addr() -> SocketAddr {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let probe = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(1));
        assert_eq!(
            probe.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::ConnectionRefused),
            "something listens on {addr}"
        );
        addr
    }

    async fn run_n(config: RunConfig) -> Summary {
        tokio::time::timeout(WAIT * 3, run(config, CancellationToken::new(), |_| {}))
            .await
            .expect("run did not finish")
    }

    fn fixed(transport: Transport, n: u64, c: usize) -> RunConfig {
        RunConfig {
            requests: Some(n),
            concurrency: c,
            ..RunConfig::new(transport)
        }
    }

    fn assert_all_ok(s: &Summary, n: u64) {
        assert_eq!(s.total, n, "{s:#?}");
        assert_eq!(s.ok, n, "{s:#?}");
        assert_eq!(s.errors, 0, "{s:#?}");
        assert_eq!(s.outages.count, 0);
        assert!(!s.interrupted);
        assert!(s.latency.is_some());
    }

    #[tokio::test]
    async fn tcp_fixed_n() {
        let (addr, server) = tcp_server(localhost()).await;
        let s = run_n(fixed(Transport::Tcp(addr), 50, 4)).await;
        assert_all_ok(&s, 50);
        assert_eq!(s.protocol, "tcp");
        assert_eq!(s.conn_mode, "persistent");

        let mut config = fixed(Transport::Tcp(addr), 20, 2);
        config.conn_mode = ConnMode::PerRequest;
        config.filler = Filler::Random;
        config.payload_size = Some(512);
        assert_all_ok(&run_n(config).await, 20);

        // Payloads larger than the default client limits still fit.
        let mut config = fixed(Transport::Tcp(addr), 2, 1);
        config.payload_size = Some(11 * 1024 * 1024);
        config.timeout = Duration::from_secs(30);
        assert_all_ok(&run_n(config).await, 2);
        server.stop().await;
    }

    #[tokio::test]
    async fn udp_fixed_n() {
        let (addr, server) = udp_server().await;
        assert_all_ok(&run_n(fixed(Transport::Udp(addr), 50, 4)).await, 50);
        let mut config = fixed(Transport::Udp(addr), 10, 2);
        config.conn_mode = ConnMode::PerRequest;
        assert_all_ok(&run_n(config).await, 10);
        server.stop().await;
    }

    #[tokio::test]
    async fn http_fixed_n() {
        let (addr, server) = http_server(None).await;
        let s = run_n(fixed(Transport::Http(addr), 50, 4)).await;
        assert_all_ok(&s, 50);
        assert_eq!(s.conn_mode, "per-request");

        // Bodies are framed by Content-Length, so large payloads echo whole.
        let mut config = fixed(Transport::Http(addr), 4, 2);
        config.payload_size = Some(512 * 1024);
        config.filler = Filler::Random;
        assert_all_ok(&run_n(config).await, 4);
        server.stop().await;
    }

    #[tokio::test]
    async fn unix_stream_fixed_n() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream.sock");
        let server = unix_stream_server(&path).await;
        assert_all_ok(&run_n(fixed(Transport::UnixStream(path), 50, 4)).await, 50);
        server.stop().await;
    }

    #[tokio::test]
    async fn unix_dgram_fixed_n() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dgram.sock");
        let server = unix_dgram_server(&path).await;
        let s = run_n(fixed(Transport::UnixDgram(path.clone()), 50, 4)).await;
        assert_all_ok(&s, 50);
        let mut config = fixed(Transport::UnixDgram(path), 10, 2);
        config.conn_mode = ConnMode::PerRequest;
        assert_all_ok(&run_n(config).await, 10);
        server.stop().await;
    }

    #[tokio::test]
    async fn refused_port_counts_attempts_and_outage() {
        let addr = refused_addr();
        for transport in [Transport::Tcp(addr), Transport::Http(addr)] {
            let mut config = fixed(transport, 20, 2);
            config.reconnect_delay = Duration::from_millis(5);
            let s = run_n(config).await;
            assert_eq!(s.total, 20);
            assert_eq!(s.ok, 0);
            assert_eq!(s.errors_by_kind["connect_refused"], 20, "{s:#?}");
            assert!((s.error_rate_pct - 100.0).abs() < 1e-9);
            assert!(s.latency.is_none());
            assert_eq!(s.outages.count, 1);
            assert!(s.outages.ongoing);
            assert!(!s.interrupted);
        }
    }

    #[tokio::test]
    async fn missing_unix_socket_is_connect_failed() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("none.sock");
        for transport in [
            Transport::UnixStream(missing.clone()),
            // Datagram clients find out when they send.
            Transport::UnixDgram(missing.clone()),
        ] {
            let mut config = fixed(transport, 4, 1);
            config.reconnect_delay = Duration::ZERO;
            let s = run_n(config).await;
            assert_eq!(s.errors_by_kind["connect_failed"], 4, "{s:#?}");
            assert_eq!(s.outages.count, 1);
        }
    }

    /// A TCP server that answers each connection's first read with `reply`
    /// (applied to what it read) and then closes the connection.
    async fn bad_echo_server(
        reply: fn(&[u8]) -> Vec<u8>,
    ) -> (SocketAddr, JoinHandle<std::io::Result<()>>) {
        let listener = tokio::net::TcpListener::bind(localhost()).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await?;
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await?;
                stream.write_all(&reply(&buf[..n])).await?;
            }
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn wrong_echo_is_a_mismatch() {
        let (addr, server) = bad_echo_server(|data| data.to_ascii_uppercase()).await;
        let mut config = fixed(Transport::Tcp(addr), 5, 1);
        config.conn_mode = ConnMode::PerRequest;
        let s = run_n(config).await;
        assert_eq!(s.errors_by_kind["mismatch"], 5, "{s:#?}");
        assert_eq!(s.mismatches(), 5);
        // A live server echoing wrong bytes is not an outage.
        assert_eq!(s.outages.count, 0, "{s:#?}");
        server.abort();
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let ms = Duration::from_millis;
        let b = |n| backoff(ms(100), ms(1000), n);
        assert_eq!(
            [b(1), b(2), b(3), b(4), b(5), b(50)],
            [ms(100), ms(200), ms(400), ms(800), ms(1000), ms(1000)]
        );
        assert_eq!(backoff(Duration::ZERO, ms(1000), 9), Duration::ZERO);
    }

    /// A server that drops every connection must not cause a reconnect
    /// storm: each worker backs off exponentially after errors.
    #[tokio::test]
    async fn errors_back_off_instead_of_reconnecting_in_a_loop() {
        let (addr, server) = bad_echo_server(|_| Vec::new()).await;
        let mut config = RunConfig::new(Transport::Tcp(addr));
        config.concurrency = 4;
        // Only the backoff limits reconnects here.
        config.conn_rate = None;
        config.reconnect_delay = Duration::from_millis(10);
        config.max_backoff = Duration::from_millis(100);
        let cancel = CancellationToken::new();
        let stop = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                cancel.cancel();
            })
        };
        let s = tokio::time::timeout(WAIT * 3, run(config, cancel, |_| {}))
            .await
            .expect("run did not finish");
        stop.await.unwrap();
        // 10+20+40+80ms, then 100ms each: about 13 attempts per worker in
        // 1s. Without the backoff it was thousands.
        assert!(s.total > 4 && s.total <= 4 * 20, "{s:#?}");
        assert_eq!(s.errors_by_kind["reset"], s.total);
        server.abort();
    }

    #[tokio::test]
    async fn new_connections_are_capped() {
        let (addr, server) = tcp_server(localhost()).await;
        let mut config = fixed(Transport::Tcp(addr), 30, 4);
        config.conn_mode = ConnMode::PerRequest;
        config.conn_rate = Some(RateLimitConfig::new(100, 1));
        let started = Instant::now();
        let s = run_n(config).await;
        assert_all_ok(&s, 30);
        // 30 connections at 100/s with a burst of 1 take at least ~0.29s.
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "{:?}",
            started.elapsed()
        );
        server.stop().await;
    }

    /// `EADDRNOTAVAIL` stops the whole run instead of being retried. On
    /// macOS, connecting to port 0 fails with it.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn ports_exhausted_stops_the_run() {
        let config = RunConfig {
            concurrency: 2,
            ..RunConfig::new(Transport::Tcp("127.0.0.1:0".parse().unwrap()))
        };
        let s = tokio::time::timeout(WAIT * 3, run(config, CancellationToken::new(), |_| {}))
            .await
            .expect("run did not stop by itself");
        assert_eq!(s.stop_reason, STOP_PORTS_EXHAUSTED);
        assert!(s.errors_by_kind["ports_exhausted"] >= 1, "{s:#?}");
        assert_eq!(s.outages.count, 0);
    }

    #[tokio::test]
    async fn short_echo_then_close_is_reset_not_ok() {
        let (addr, server) = bad_echo_server(|data| data[..data.len() / 2].to_vec()).await;
        let mut config = fixed(Transport::Tcp(addr), 5, 1);
        config.conn_mode = ConnMode::PerRequest;
        let s = run_n(config).await;
        assert_eq!(s.ok, 0, "{s:#?}");
        assert_eq!(s.errors_by_kind["reset"], 5, "{s:#?}");
        assert_eq!(s.mismatches(), 0);
        server.abort();
    }

    /// Waits for the first event matching `pred`, keeping every event seen.
    async fn wait_for_event(
        rx: &mut mpsc::UnboundedReceiver<LiveEvent>,
        seen: &mut Vec<LiveEvent>,
        what: &str,
        pred: impl Fn(&LiveEvent) -> bool,
    ) {
        tokio::time::timeout(WAIT, async {
            while let Some(event) = rx.recv().await {
                let hit = pred(&event);
                seen.push(event);
                if hit {
                    return;
                }
            }
            panic!("run ended before {what}");
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// Restarts a TCP server under a continuous run: shut it down (closing
    /// the listener and cancelling connections), keep it down for
    /// `DOWNTIME`, then bind the same address again. The client must see
    /// exactly one outage covering the downtime.
    async fn restart_produces_one_outage(conn_mode: ConnMode) {
        const DOWNTIME: Duration = Duration::from_millis(300);
        let (addr, server) = tcp_server(localhost()).await;

        let config = RunConfig {
            concurrency: 2,
            conn_mode,
            // Bounded so per-request mode does not pile up TIME_WAIT sockets.
            rate: Some(RateLimitConfig::new(200, 1)),
            reconnect_delay: Duration::from_millis(20),
            timeout: Duration::from_secs(1),
            interval: Some(Duration::from_millis(100)),
            ..RunConfig::new(Transport::Tcp(addr))
        };
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let run_task = tokio::spawn(run(config, cancel.clone(), move |e| {
            let _ = tx.send(e);
        }));
        let mut events = Vec::new();

        wait_for_event(
            &mut rx,
            &mut events,
            "traffic",
            |e| matches!(e, LiveEvent::Interval(r) if r.window.ok > 0),
        )
        .await;
        server.stop().await;
        wait_for_event(&mut rx, &mut events, "outage start", |e| {
            matches!(e, LiveEvent::Outage(OutageEvent::Started { .. }))
        })
        .await;
        tokio::time::sleep(DOWNTIME).await;
        let (rebound, server) = tcp_server(addr).await;
        assert_eq!(rebound, addr);
        wait_for_event(&mut rx, &mut events, "outage end", |e| {
            matches!(e, LiveEvent::Outage(OutageEvent::Ended { .. }))
        })
        .await;
        wait_for_event(
            &mut rx,
            &mut events,
            "an interval after recovery",
            |e| matches!(e, LiveEvent::Interval(r) if r.window.ok > 0 && !r.outage_open),
        )
        .await;
        cancel.cancel();
        let s = tokio::time::timeout(WAIT, run_task)
            .await
            .expect("run did not stop after cancel")
            .unwrap();
        events.extend(std::iter::from_fn(|| rx.try_recv().ok()));
        server.stop().await;

        let mode = conn_mode.as_str();
        assert!(s.ok > 0, "{mode}: {s:#?}");
        assert!(s.errors_by_kind["connect_refused"] > 0, "{mode}: {s:#?}");
        assert_eq!(s.outages.count, 1, "{mode}: {s:#?}");
        assert!(!s.outages.ongoing, "{mode}: {s:#?}");
        let min_ms = DOWNTIME.as_secs_f64() * 1000.0;
        assert!(s.outages.longest_ms >= min_ms, "{mode}: {s:#?}");
        assert!(s.outages.longest_ms < 5000.0, "{mode}: {s:#?}");
        assert!(!s.interrupted, "continuous runs are not 'interrupted'");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LiveEvent::Interval(r) if r.outage_open)),
            "{mode}: no interval was marked OUTAGE"
        );
    }

    #[tokio::test]
    async fn server_restart_persistent_mode() {
        restart_produces_one_outage(ConnMode::Persistent).await;
    }

    #[tokio::test]
    async fn server_restart_per_request_mode() {
        restart_produces_one_outage(ConnMode::PerRequest).await;
    }

    #[tokio::test]
    async fn unix_stream_restart_persistent_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restart.sock");
        let server = unix_stream_server(&path).await;
        let config = RunConfig {
            rate: Some(RateLimitConfig::new(200, 1)),
            reconnect_delay: Duration::from_millis(10),
            interval: Some(Duration::from_millis(100)),
            ..RunConfig::new(Transport::UnixStream(path.clone()))
        };
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let run_task = tokio::spawn(run(config, cancel.clone(), move |e| {
            let _ = tx.send(e);
        }));
        let mut events = Vec::new();
        wait_for_event(
            &mut rx,
            &mut events,
            "traffic",
            |e| matches!(e, LiveEvent::Interval(r) if r.window.ok > 0),
        )
        .await;
        server.stop().await; // also removes the socket file
        wait_for_event(&mut rx, &mut events, "outage start", |e| {
            matches!(e, LiveEvent::Outage(OutageEvent::Started { .. }))
        })
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let server = unix_stream_server(&path).await;
        wait_for_event(&mut rx, &mut events, "outage end", |e| {
            matches!(e, LiveEvent::Outage(OutageEvent::Ended { .. }))
        })
        .await;
        cancel.cancel();
        let s = tokio::time::timeout(WAIT, run_task).await.unwrap().unwrap();
        server.stop().await;

        assert_eq!(s.outages.count, 1, "{s:#?}");
        assert!(s.outages.longest_ms >= 200.0, "{s:#?}");
        // The removed socket file shows up as a missing path.
        assert!(s.errors_by_kind["connect_failed"] > 0, "{s:#?}");
    }

    /// A TCP echo server that waits `delay` before each reply. It sends `()`
    /// on the returned channel as each request arrives.
    async fn slow_echo_server(
        delay: Duration,
    ) -> (SocketAddr, mpsc::UnboundedReceiver<()>, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind(localhost()).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        let _ = tx.send(());
                        tokio::time::sleep(delay).await;
                        if stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (addr, rx, handle)
    }

    #[tokio::test]
    async fn stop_lets_requests_in_flight_finish() {
        let delay = Duration::from_millis(300);
        let (addr, mut requests, server) = slow_echo_server(delay).await;
        let mut config = RunConfig::new(Transport::Tcp(addr));
        config.concurrency = 2;
        let cancel = CancellationToken::new();
        let started = Instant::now();
        let run_task = tokio::spawn(run(config, cancel.clone(), |_| {}));
        // Cancel once both workers' first requests are waiting for their
        // replies.
        tokio::time::timeout(WAIT, async {
            for _ in 0..2 {
                requests.recv().await.unwrap();
            }
        })
        .await
        .expect("both requests did not reach the server");
        cancel.cancel();
        let s = tokio::time::timeout(WAIT, run_task)
            .await
            .expect("run did not stop")
            .unwrap();
        assert_eq!((s.total, s.ok), (2, 2), "{s:#?}");
        assert!(started.elapsed() >= delay, "{:?}", started.elapsed());
        server.abort();
    }

    #[tokio::test]
    async fn cancel_before_n_marks_interrupted() {
        let (addr, server) = tcp_server(localhost()).await;
        let config = RunConfig {
            rate: Some(RateLimitConfig::new(20, 1)),
            ..fixed(Transport::Tcp(addr), 1000, 2)
        };
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let run_task = tokio::spawn(run(
            RunConfig {
                interval: Some(Duration::from_millis(50)),
                ..config
            },
            cancel.clone(),
            move |e| {
                let _ = tx.send(e);
            },
        ));
        let mut events = Vec::new();
        wait_for_event(
            &mut rx,
            &mut events,
            "traffic",
            |e| matches!(e, LiveEvent::Interval(r) if r.window.ok > 0),
        )
        .await;
        cancel.cancel();
        let s = tokio::time::timeout(WAIT, run_task).await.unwrap().unwrap();
        assert!(s.interrupted);
        assert!(s.total > 0 && s.total < 1000, "{s:#?}");
        assert_eq!(s.errors, 0, "{s:#?}");
        server.stop().await;
    }

    #[tokio::test]
    async fn shaper_paces_requests() {
        let (addr, server) = tcp_server(localhost()).await;
        let config = RunConfig {
            rate: Some(RateLimitConfig::new(100, 1)),
            ..fixed(Transport::Tcp(addr), 50, 4)
        };
        let started = std::time::Instant::now();
        let s = run_n(config).await;
        let elapsed = started.elapsed();
        assert_all_ok(&s, 50);
        // 49 intervals of 10ms after the first token.
        assert!(
            elapsed >= Duration::from_millis(450) && elapsed <= Duration::from_secs(5),
            "50 requests at 100/s took {elapsed:?}"
        );
        server.stop().await;
    }

    #[tokio::test]
    async fn server_rate_limit_is_not_an_outage() {
        let (addr, server) = http_server(Some(RateLimitConfig::new(10, 10))).await;
        // 60 requests at 100/s against a server admitting 10/s plus a burst
        // of 10: the excess gets 429. (Shaped rather than unbounded: every
        // HTTP request is a new connection, and an unbounded flood exhausts
        // ephemeral ports via TIME_WAIT.)
        let config = RunConfig {
            rate: Some(RateLimitConfig::new(100, 8)),
            ..fixed(Transport::Http(addr), 60, 8)
        };
        let s = run_n(config).await;
        assert!(s.ok >= 10, "{s:#?}");
        assert!(s.errors_by_kind["rate_limited"] > 0, "{s:#?}");
        assert_eq!(s.errors, s.errors_by_kind["rate_limited"], "{s:#?}");
        assert_eq!(s.outages.count, 0, "{s:#?}");
        server.stop().await;
    }

    #[tokio::test]
    async fn honor_retry_after_waits() {
        let (addr, server) = http_server(Some(RateLimitConfig::new(1, 1))).await;
        // Retry-After is rounded up to 1s, so with one worker the 429 on the
        // second request delays the third by about a second, which the
        // server then admits.
        let config = RunConfig {
            honor_retry_after: true,
            ..fixed(Transport::Http(addr), 3, 1)
        };
        let started = std::time::Instant::now();
        let s = run_n(config).await;
        assert_eq!(s.errors_by_kind["rate_limited"], 1, "{s:#?}");
        assert_eq!(s.ok, 2, "{s:#?}");
        assert!(started.elapsed() >= Duration::from_millis(900));
        server.stop().await;
    }
}
