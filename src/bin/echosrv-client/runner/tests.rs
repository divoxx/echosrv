use super::*;
use crate::stats::OutageEvent;
use crate::test_common::{
    refused_addr, slow_echo_server, socket_dir, start_http, start_tcp, start_udp,
    start_unix_datagram_at, start_unix_stream_at,
};
use echosrv::{HttpConfig, TcpConfig, UdpConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
// readiness probes and no serialization between tests. The helpers are
// shared with the integration tests (`tests/common/mod.rs`).

/// A stop reason nobody has set yet.
fn unset() -> Arc<OnceLock<StopReason>> {
    Arc::new(OnceLock::new())
}

async fn run_n(config: RunConfig) -> Summary {
    tokio::time::timeout(
        WAIT * 3,
        run(config, CancellationToken::new(), unset(), |_| {}),
    )
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
    assert_eq!(s.stats.total, n, "{s:#?}");
    assert_eq!(s.stats.ok, n, "{s:#?}");
    assert_eq!(s.stats.errors, 0, "{s:#?}");
    assert_eq!(s.stats.outages.count, 0);
    assert!(!s.interrupted);
    assert_eq!(s.stop_reason, StopReason::Completed);
    assert!(s.stats.latency.is_some());
}

#[tokio::test]
async fn tcp_fixed_n() {
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;
    let s = run_n(fixed(Transport::Tcp(addr), 50, 4)).await;
    assert_all_ok(&s, 50);
    assert_eq!(s.info.protocol, "tcp");
    assert_eq!(s.info.conn_mode, "persistent");

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
    let server = start_udp(UdpConfig::default()).await;
    let addr = server.addr;
    assert_all_ok(&run_n(fixed(Transport::Udp(addr), 50, 4)).await, 50);
    let mut config = fixed(Transport::Udp(addr), 10, 2);
    config.conn_mode = ConnMode::PerRequest;
    assert_all_ok(&run_n(config).await, 10);
    server.stop().await;
}

#[tokio::test]
async fn http_fixed_n() {
    let server = start_http(HttpConfig::default()).await;
    let addr = server.addr;
    let s = run_n(fixed(Transport::Http(addr), 50, 4)).await;
    assert_all_ok(&s, 50);
    assert_eq!(s.info.conn_mode, "per-request");

    // Bodies are framed by Content-Length, so large payloads echo whole.
    let mut config = fixed(Transport::Http(addr), 4, 2);
    config.payload_size = Some(512 * 1024);
    config.filler = Filler::Random;
    assert_all_ok(&run_n(config).await, 4);
    server.stop().await;
}

#[tokio::test]
async fn unix_stream_fixed_n() {
    let dir = socket_dir();
    let path = dir.path().join("stream.sock");
    let server = start_unix_stream_at(&path).await;
    assert_all_ok(&run_n(fixed(Transport::UnixStream(path), 50, 4)).await, 50);
    server.stop().await;
}

#[tokio::test]
async fn unix_dgram_fixed_n() {
    let dir = socket_dir();
    let path = dir.path().join("dgram.sock");
    let server = start_unix_datagram_at(&path).await;
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
        assert_eq!(s.stats.total, 20);
        assert_eq!(s.stats.ok, 0);
        assert_eq!(
            s.stats.errors_by_kind[&ErrorKind::ConnectRefused],
            20,
            "{s:#?}"
        );
        assert!((s.stats.error_rate_pct - 100.0).abs() < 1e-9);
        assert!(s.stats.latency.is_none());
        assert_eq!(s.stats.outages.count, 1);
        assert!(s.stats.outages.ongoing);
        assert!(!s.interrupted);
    }
}

#[tokio::test]
async fn missing_unix_socket_is_connect_failed() {
    let dir = socket_dir();
    let missing = dir.path().join("none.sock");
    for transport in [
        Transport::UnixStream(missing.clone()),
        // Datagram clients find out when they send.
        Transport::UnixDgram(missing.clone()),
    ] {
        let mut config = fixed(transport, 4, 1);
        config.reconnect_delay = Duration::ZERO;
        let s = run_n(config).await;
        assert_eq!(
            s.stats.errors_by_kind[&ErrorKind::ConnectFailed],
            4,
            "{s:#?}"
        );
        assert_eq!(s.stats.outages.count, 1);
    }
}

/// A TCP server that answers each connection's first read with `reply`
/// (applied to what it read) and then closes the connection.
async fn bad_echo_server(
    reply: fn(&[u8]) -> Vec<u8>,
) -> (SocketAddr, JoinHandle<std::io::Result<()>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
    assert_eq!(s.stats.errors_by_kind[&ErrorKind::Mismatch], 5, "{s:#?}");
    assert_eq!(s.stats.mismatches(), 5);
    // A live server echoing wrong bytes is not an outage.
    assert_eq!(s.stats.outages.count, 0, "{s:#?}");
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
    let s = tokio::time::timeout(WAIT * 3, run(config, cancel, unset(), |_| {}))
        .await
        .expect("run did not finish");
    stop.await.unwrap();
    // 10+20+40+80ms, then 100ms each: about 13 attempts per worker in
    // 1s. Without the backoff it was thousands.
    assert!(s.stats.total > 4 && s.stats.total <= 4 * 20, "{s:#?}");
    assert_eq!(s.stats.errors_by_kind[&ErrorKind::Reset], s.stats.total);
    server.abort();
}

#[tokio::test]
async fn new_connections_are_capped() {
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;
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
    let s = tokio::time::timeout(
        WAIT * 3,
        run(config, CancellationToken::new(), unset(), |_| {}),
    )
    .await
    .expect("run did not stop by itself");
    assert_eq!(s.stop_reason, StopReason::PortsExhausted);
    assert!(
        s.stats.errors_by_kind[&ErrorKind::PortsExhausted] >= 1,
        "{s:#?}"
    );
    assert_eq!(s.stats.outages.count, 0);
}

#[tokio::test]
async fn short_echo_then_close_is_reset_not_ok() {
    let (addr, server) = bad_echo_server(|data| data[..data.len() / 2].to_vec()).await;
    let mut config = fixed(Transport::Tcp(addr), 5, 1);
    config.conn_mode = ConnMode::PerRequest;
    let s = run_n(config).await;
    assert_eq!(s.stats.ok, 0, "{s:#?}");
    assert_eq!(s.stats.errors_by_kind[&ErrorKind::Reset], 5, "{s:#?}");
    assert_eq!(s.stats.mismatches(), 0);
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
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;

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
    let run_task = tokio::spawn(run(config, cancel.clone(), unset(), move |e| {
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
    let server = start_tcp(TcpConfig {
        bind_addr: addr,
        ..Default::default()
    })
    .await;
    assert_eq!(server.addr, addr);
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
    assert!(s.stats.ok > 0, "{mode}: {s:#?}");
    assert!(
        s.stats.errors_by_kind[&ErrorKind::ConnectRefused] > 0,
        "{mode}: {s:#?}"
    );
    assert_eq!(s.stats.outages.count, 1, "{mode}: {s:#?}");
    assert!(!s.stats.outages.ongoing, "{mode}: {s:#?}");
    let min_ms = DOWNTIME.as_secs_f64() * 1000.0;
    assert!(s.stats.outages.longest_ms >= min_ms, "{mode}: {s:#?}");
    assert!(s.stats.outages.longest_ms < 5000.0, "{mode}: {s:#?}");
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
    let dir = socket_dir();
    let path = dir.path().join("restart.sock");
    let server = start_unix_stream_at(&path).await;
    let config = RunConfig {
        rate: Some(RateLimitConfig::new(200, 1)),
        reconnect_delay: Duration::from_millis(10),
        interval: Some(Duration::from_millis(100)),
        ..RunConfig::new(Transport::UnixStream(path.clone()))
    };
    let cancel = CancellationToken::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let run_task = tokio::spawn(run(config, cancel.clone(), unset(), move |e| {
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
    let server = start_unix_stream_at(&path).await;
    wait_for_event(&mut rx, &mut events, "outage end", |e| {
        matches!(e, LiveEvent::Outage(OutageEvent::Ended { .. }))
    })
    .await;
    cancel.cancel();
    let s = tokio::time::timeout(WAIT, run_task).await.unwrap().unwrap();
    server.stop().await;

    assert_eq!(s.stats.outages.count, 1, "{s:#?}");
    assert!(s.stats.outages.longest_ms >= 200.0, "{s:#?}");
    // The removed socket file shows up as a missing path.
    assert!(
        s.stats.errors_by_kind[&ErrorKind::ConnectFailed] > 0,
        "{s:#?}"
    );
}

#[tokio::test]
async fn stop_lets_requests_in_flight_finish() {
    let delay = Duration::from_millis(300);
    let (addr, mut requests, server) = slow_echo_server(delay).await;
    let mut config = RunConfig::new(Transport::Tcp(addr));
    config.concurrency = 2;
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let run_task = tokio::spawn(run(config, cancel.clone(), unset(), |_| {}));
    // Both workers' first requests are waiting for their replies.
    for _ in 0..2 {
        tokio::time::timeout(WAIT, requests.recv())
            .await
            .expect("a request did not reach the server");
    }
    cancel.cancel();
    let s = tokio::time::timeout(WAIT, run_task)
        .await
        .expect("run did not stop")
        .unwrap();
    assert_eq!((s.stats.total, s.stats.ok), (2, 2), "{s:#?}");
    assert!(started.elapsed() >= delay, "{:?}", started.elapsed());
    server.abort();
}

#[tokio::test]
async fn cancel_before_n_marks_interrupted() {
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;
    let config = RunConfig {
        rate: Some(RateLimitConfig::new(20, 1)),
        ..fixed(Transport::Tcp(addr), 1000, 2)
    };
    let cancel = CancellationToken::new();
    let reason = unset();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let run_task = tokio::spawn(run(
        RunConfig {
            interval: Some(Duration::from_millis(50)),
            ..config
        },
        cancel.clone(),
        reason.clone(),
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
    stop(&cancel, &reason, StopReason::Interrupt);
    let s = tokio::time::timeout(WAIT, run_task).await.unwrap().unwrap();
    assert!(s.interrupted);
    assert_eq!(s.stop_reason, StopReason::Interrupt);
    assert!(s.stats.total > 0 && s.stats.total < 1000, "{s:#?}");
    assert_eq!(s.stats.errors, 0, "{s:#?}");
    server.stop().await;
}

#[test]
fn first_stop_reason_wins() {
    let cancel = CancellationToken::new();
    let reason = OnceLock::new();
    stop(&cancel, &reason, StopReason::Terminated);
    assert!(cancel.is_cancelled());
    stop(&cancel, &reason, StopReason::PortsExhausted);
    assert_eq!(reason.get(), Some(&StopReason::Terminated));
}

#[tokio::test]
async fn duration_stops_the_run() {
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;
    let duration = Duration::from_millis(300);
    let config = RunConfig {
        rate: Some(RateLimitConfig::new(50, 1)),
        duration: Some(duration),
        ..RunConfig::new(Transport::Tcp(addr))
    };
    let cancel = CancellationToken::new();
    let started = std::time::Instant::now();
    let s = tokio::time::timeout(WAIT, run(config, cancel.clone(), unset(), |_| {}))
        .await
        .expect("run did not stop at --duration");
    assert!(started.elapsed() >= duration, "{:?}", started.elapsed());
    assert!(cancel.is_cancelled());
    assert_eq!(s.stop_reason, StopReason::Duration);
    assert!(!s.interrupted, "continuous runs are not 'interrupted'");
    assert!(s.stats.ok > 0 && s.stats.errors == 0, "{s:#?}");
    server.stop().await;
}

#[tokio::test]
async fn completed_run_is_not_stopped_by_its_duration() {
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;
    let config = RunConfig {
        duration: Some(Duration::from_millis(200)),
        ..fixed(Transport::Tcp(addr), 5, 1)
    };
    let cancel = CancellationToken::new();
    let s = tokio::time::timeout(WAIT, run(config, cancel.clone(), unset(), |_| {}))
        .await
        .expect("run did not finish");
    assert_all_ok(&s, 5);
    // The timer went with the run: it never fires afterwards.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!cancel.is_cancelled());
    server.stop().await;
}

#[tokio::test]
async fn shaper_paces_requests() {
    let server = start_tcp(TcpConfig::default()).await;
    let addr = server.addr;
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
    let server =
        start_http(HttpConfig::default().with_rate_limit(RateLimitConfig::new(10, 10))).await;
    let addr = server.addr;
    // 60 requests at 100/s against a server admitting 10/s plus a burst
    // of 10: the excess gets 429. (Shaped rather than unbounded: every
    // HTTP request is a new connection; see `SAFE_CONN_RATE`.)
    let config = RunConfig {
        rate: Some(RateLimitConfig::new(100, 8)),
        ..fixed(Transport::Http(addr), 60, 8)
    };
    let s = run_n(config).await;
    assert!(s.stats.ok >= 10, "{s:#?}");
    assert!(
        s.stats.errors_by_kind[&ErrorKind::RateLimited] > 0,
        "{s:#?}"
    );
    assert_eq!(
        s.stats.errors,
        s.stats.errors_by_kind[&ErrorKind::RateLimited],
        "{s:#?}"
    );
    assert_eq!(s.stats.outages.count, 0, "{s:#?}");
    server.stop().await;
}

#[tokio::test]
async fn honor_retry_after_waits() {
    let server =
        start_http(HttpConfig::default().with_rate_limit(RateLimitConfig::new(1, 1))).await;
    let addr = server.addr;
    // Retry-After is rounded up to 1s, so with one worker the 429 on the
    // second request delays the third by about a second, which the
    // server then admits.
    let config = RunConfig {
        honor_retry_after: true,
        ..fixed(Transport::Http(addr), 3, 1)
    };
    let started = std::time::Instant::now();
    let s = run_n(config).await;
    assert_eq!(s.stats.errors_by_kind[&ErrorKind::RateLimited], 1, "{s:#?}");
    assert_eq!(s.stats.ok, 2, "{s:#?}");
    assert!(started.elapsed() >= Duration::from_millis(900));
    server.stop().await;
}
