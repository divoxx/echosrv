//! Black-box tests for the `echosrv-client` binary.
//!
//! Servers run in-process and are bound before the client starts (port 0 or
//! a fresh socket path), so there are no port races and no readiness probes.
//! "Refused" targets use `127.0.0.1:1`, a privileged port nothing listens on.
//!
//! Like `tests/cli.rs`, the tests are serialized ([`serial`]). On macOS,
//! std sets `FD_CLOEXEC` on a new socket in a separate syscall, so a child
//! process spawned at that moment by another test can inherit the socket and
//! keep a port, connection or socket path alive after its owner closed it.

mod common;

use assert_cmd::Command as SyncCommand;
use common::{
    socket_dir, start_http, start_tcp, start_udp, start_unix_datagram_at, start_unix_stream_at,
};
use echosrv::http::HttpConfig;
use echosrv::{RateLimitConfig, TcpConfig, UdpConfig};
use predicates::prelude::*;
use std::net::SocketAddr;
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_echosrv-client");
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// The lock behind [`serial`] / [`serial_blocking`]. A Tokio mutex, because
/// async tests hold it across `.await`.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes the tests in this file (see the module docs).
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}

/// [`serial`] for synchronous tests.
fn serial_blocking() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.blocking_lock()
}

/// An address nothing listens on (see the module docs).
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

async fn run_client(args: &[&str]) -> Output {
    run_client_env(args, &[]).await
}

/// Runs the client with stdout/stderr piped, the color env vars cleared and
/// `env` applied on top.
async fn run_client_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("RUST_LOG")
        .envs(env.iter().copied())
        .kill_on_drop(true);
    tokio::time::timeout(RUN_TIMEOUT, cmd.output())
        .await
        .expect("echosrv-client timed out")
        .expect("failed to run echosrv-client")
}

fn describe(out: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn json_lines(out: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("non-JSON line {line:?}: {e}"))
        })
        .collect()
}

fn has_ansi(bytes: &[u8]) -> bool {
    bytes.windows(2).any(|w| w == b"\x1b[")
}

// ---------------------------------------------------------------------------
// Arguments and help
// ---------------------------------------------------------------------------

#[test]
fn help_states_every_default() {
    let _serial = serial_blocking();
    let defaults = [
        "[default: tcp]",
        "[default: 127.0.0.1:8080 for tcp/udp/http",
        "/tmp/echosrv_stream.sock for unix-stream",
        "/tmp/echosrv_datagram.sock for unix-dgram]",
        "[default: unlimited",
        "[default: 1]",
        "[default: none]",
        "[default: unshaped]",
        "[default: 64, or header + TEXT with --payload]",
        "[default: 5s]",
        "[default: persistent; per-request for http]",
        "[default: 100ms]",
        "[default: 1s]",
        "[default: auto]",
        "[default: 0]",
    ];
    for flag in ["-h", "--help"] {
        let output = SyncCommand::new(BIN).arg(flag).output().unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        for default in defaults {
            assert!(help.contains(default), "{flag} lacks {default:?}:\n{help}");
        }
        assert!(help.contains("Exit codes:"), "{help}");
        if flag == "--help" {
            assert!(help.contains("unix-datagram"), "{help}");
            for line in help.lines() {
                if let Some(at) = line.find("[default: ") {
                    assert!(
                        line[..at].trim().is_empty(),
                        "default not on its own line: {line:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn version_prints_crate_version() {
    let _serial = serial_blocking();
    SyncCommand::new(BIN)
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("echosrv-client {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn usage_errors_exit_2() {
    let _serial = serial_blocking();
    for args in [
        &["tcp", "127.0.0.1:1", "--burst", "5"][..],
        &["quic"],
        &["tcp", "127.0.0.1:1", "-c", "0"],
        &["tcp", "127.0.0.1:1", "--color", "sometimes"],
        &["http", "127.0.0.1:1", "--conn-mode", "persistent"],
        &["tcp", "no-such-host.invalid:80", "-n", "1"],
        &["tcp", "nope"],
    ] {
        SyncCommand::new(BIN)
            .args(args)
            .env_remove("NO_COLOR")
            .assert()
            .code(2);
    }
    // Setup errors are tagged on stderr.
    SyncCommand::new(BIN)
        .args(["tcp", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::starts_with(
            "[fail] tcp target must be HOST:PORT or PORT",
        ));
    SyncCommand::new(BIN)
        .args(["http", "127.0.0.1:1", "--conn-mode", "persistent"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "the server closes every connection after one response",
        ));
}

// ---------------------------------------------------------------------------
// Runs against real servers
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn tcp_fixed_n_json_lines() {
    let _serial = serial().await;
    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();

    let out = run_client(&["tcp", &target, "-n", "20", "-c", "2", "--json"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    // With --json, every stdout line is JSON; the configuration comes first
    // and the summary last.
    let lines = json_lines(&out);
    let config = &lines[0];
    assert_eq!(config["type"], "config");
    assert_eq!(config["protocol"], "tcp");
    assert_eq!(config["target"], target);
    assert_eq!(config["requests"], 20);
    assert_eq!(config["concurrency"], 2);
    assert_eq!(config["conn_mode"], "persistent");
    let defaults = config["defaults"].as_array().unwrap();
    assert!(defaults.contains(&"timeout_s".into()), "{config}");
    assert!(defaults.contains(&"conn_mode".into()), "{config}");
    assert!(!defaults.contains(&"concurrency".into()), "{config}");
    assert!(!defaults.contains(&"target".into()), "{config}");

    let summary = lines.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["ok"], 20);
    assert_eq!(summary["total"], 20);
    assert_eq!(summary["errors"], 0);
    assert_eq!(summary["interrupted"], false);
    assert_eq!(summary["stop_reason"], "completed");
    assert_eq!(summary["outages"]["count"], 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_protocol_echoes() {
    let _serial = serial().await;
    let dir = socket_dir();
    let tcp = start_tcp(TcpConfig::default()).await;
    let udp = start_udp(UdpConfig::default()).await;
    let http = start_http(HttpConfig::default()).await;
    let stream_path = dir.path().join("s.sock");
    let dgram_path = dir.path().join("d.sock");
    let unix_stream = start_unix_stream_at(&stream_path).await;
    let unix_dgram = start_unix_datagram_at(&dgram_path).await;

    let cases = [
        ("tcp", tcp.addr.to_string(), "persistent"),
        ("udp", udp.addr.to_string(), "persistent"),
        ("http", http.addr.to_string(), "per-request"),
        (
            "unix-stream",
            stream_path.display().to_string(),
            "persistent",
        ),
        ("unix-dgram", dgram_path.display().to_string(), "persistent"),
        // The server's alias for unix-dgram.
        (
            "unix-datagram",
            dgram_path.display().to_string(),
            "persistent",
        ),
    ];
    for (protocol, target, conn_mode) in cases {
        let out = run_client(&[
            protocol, &target, "-n", "30", "-c", "3", "-s", "200", "--json",
        ])
        .await;
        assert_eq!(out.status.code(), Some(0), "{protocol}: {}", describe(&out));
        let lines = json_lines(&out);
        assert_eq!(lines[0]["target"], target, "{protocol}");
        assert_eq!(lines[0]["conn_mode"], conn_mode, "{protocol}");
        let summary = lines.last().unwrap();
        assert_eq!(summary["ok"], 30, "{protocol}: {summary}");
        assert!(summary["latency"].is_object(), "{protocol}");
    }

    // Per-request mode for the stream protocols, and an HTTP body far over
    // the old 1 KiB limit (the server frames by Content-Length).
    for (protocol, target) in [
        ("tcp", tcp.addr.to_string()),
        ("unix-stream", stream_path.display().to_string()),
    ] {
        let out = run_client(&[
            protocol,
            &target,
            "-n",
            "10",
            "--conn-mode",
            "per-request",
            "-i",
            "0",
        ])
        .await;
        assert_eq!(out.status.code(), Some(0), "{protocol}: {}", describe(&out));
    }
    let out = run_client(&[
        "http",
        &http.addr.to_string(),
        "-n",
        "4",
        "-s",
        "200000",
        "--random",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(out.stderr.is_empty(), "{}", describe(&out));

    for server in [tcp, udp, http] {
        server.stop().await;
    }
    unix_stream.stop().await;
    unix_dgram.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_exits_1_with_one_outage() {
    let _serial = serial().await;
    let target = refused_addr().to_string();
    let out = run_client(&["tcp", &target, "-n", "5", "--reconnect-delay", "10ms"]).await;
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("connect_refused=5"), "{stdout}");
    assert!(stdout.contains("outages     1"), "{stdout}");
    assert!(
        stdout
            .lines()
            .any(|l| l.ends_with("[fail] outage started (connect_refused)")),
        "{stdout}"
    );
    assert!(
        stdout
            .trim_end()
            .ends_with("[fail] error rate 100.00% above --max-error-rate 0%"),
        "{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_unix_socket_is_connect_failed() {
    let _serial = serial().await;
    let dir = socket_dir();
    let path = dir.path().join("none.sock");
    let path = path.to_str().unwrap();
    for protocol in ["unix-stream", "unix-dgram"] {
        let out = run_client(&[
            protocol,
            path,
            "-n",
            "3",
            "--reconnect-delay",
            "0",
            "--json",
        ])
        .await;
        assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
        let summary = json_lines(&out).pop().unwrap();
        assert_eq!(
            summary["errors_by_kind"]["connect_failed"], 3,
            "{protocol}: {summary}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn max_error_rate_allows_errors() {
    let _serial = serial().await;
    let target = refused_addr().to_string();
    let out = run_client(&[
        "tcp",
        &target,
        "-n",
        "2",
        "--reconnect-delay",
        "0",
        "--max-error-rate",
        "100",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
}

#[tokio::test(flavor = "multi_thread")]
async fn duration_stops_continuous_run() {
    let _serial = serial().await;
    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();
    let started = std::time::Instant::now();
    let out = run_client(&[
        "tcp", &target, "-d", "500ms", "-c", "2", "-i", "200ms", "--json",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(started.elapsed() < Duration::from_secs(10));
    let lines = json_lines(&out);
    let interval = lines
        .iter()
        .find(|l| l["type"] == "interval")
        .expect("no interval line");
    assert_eq!(interval["outage_open"], false);
    let summary = lines.last().unwrap();
    assert_eq!(summary["stop_reason"], "duration");
    assert_eq!(summary["interrupted"], false);
    assert!(summary["ok"].as_u64().unwrap() > 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http_rate_limit_is_counted_not_an_outage() {
    let _serial = serial().await;
    let server =
        start_http(HttpConfig::default().with_rate_limit(RateLimitConfig::new(1, 1))).await;
    let target = server.addr.to_string();

    // Without --honor-retry-after the extra requests get 429 right away.
    let out = run_client(&["http", &target, "-n", "4", "-i", "0", "--json"]).await;
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let summary = json_lines(&out).pop().unwrap();
    assert_eq!(summary["errors_by_kind"]["rate_limited"], 3, "{summary}");
    assert_eq!(summary["outages"]["count"], 0, "{summary}");

    // With it, the worker waits out Retry-After (>= 1s) and gets through.
    let started = std::time::Instant::now();
    let out = run_client(&[
        "http",
        &target,
        "-n",
        "2",
        "-i",
        "0",
        "--honor-retry-after",
        "--max-error-rate",
        "100",
        "--json",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let summary = json_lines(&out).pop().unwrap();
    assert!(summary["ok"].as_u64().unwrap() >= 1, "{summary}");
    assert!(started.elapsed() >= Duration::from_millis(900));
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Human output and colors
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn piped_output_is_uncolored_by_default() {
    let _serial = serial().await;
    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();
    // A bare port expands to 127.0.0.1:PORT.
    let port = server.addr.port().to_string();
    let out = run_client(&["tcp", &port, "-n", "10", "-i", "1ms"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!has_ansi(&out.stdout), "{stdout:?}");
    assert!(!has_ansi(&out.stderr));
    // Configuration header, then a blank line before the live output.
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("--- echosrv-client ---"), "{stdout}");
    assert_eq!(
        lines.next(),
        Some(format!("target      tcp {target}").as_str())
    );
    for expected in [
        "workers     1 (default), persistent connections (default)\n",
        "requests    10\n",
        "rate        unshaped (default), at most 100 new connections/s (default)\n",
        "payload     64 bytes (default), pattern filler (default)\n",
        "timeout     5s (default), backoff 100ms (default) up to 200ms (default)\n",
        "interval    1ms\n",
        "max errors  0% (default)\n\n",
        "--- echosrv-client summary ---\n",
        "outages     none\n",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
    assert!(
        stdout
            .trim_end()
            .ends_with("  [ok] no mismatches, error rate 0.00% within --max-error-rate 0%"),
        "{stdout}"
    );

    // CLICOLOR_FORCE colors piped output; NO_COLOR alone does not.
    let out = run_client_env(&["tcp", &port, "-n", "5"], &[("CLICOLOR_FORCE", "1")]).await;
    assert!(has_ansi(&out.stdout));
    let out = run_client_env(&["tcp", &port, "-n", "5"], &[("NO_COLOR", "1")]).await;
    assert!(!has_ansi(&out.stdout));
    let out = run_client_env(
        &["tcp", &port, "-n", "5", "--color", "never"],
        &[("CLICOLOR_FORCE", "1")],
    )
    .await;
    assert!(!has_ansi(&out.stdout));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn color_always_and_json() {
    let _serial = serial().await;
    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();

    let out = run_client(&["tcp", &target, "-n", "10", "--color", "always"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(has_ansi(&out.stdout), "{}", describe(&out));

    // JSON on stdout is never colored, even when forced.
    let out = run_client_env(
        &[
            "tcp", &target, "-n", "10", "-i", "1ms", "--json", "--color", "always",
        ],
        &[("CLICOLOR_FORCE", "1")],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(!has_ansi(&out.stdout));
    let lines = json_lines(&out);
    assert_eq!(lines[0]["type"], "config");
    assert_eq!(lines.last().unwrap()["ok"], 10);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn warnings_are_tagged_on_stderr() {
    let _serial = serial().await;
    let server = start_udp(UdpConfig::default()).await;
    let target = server.addr.to_string();
    let out = run_client(&["udp", &target, "-n", "1", "--honor-retry-after"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("[warn] --honor-retry-after"), "{stderr}");

    let out = run_client(&["http", "127.0.0.1:1", "-n", "1", "-s", "2000000", "-i", "0"]).await;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("[warn] http payload of 2000000 bytes"),
        "{stderr}"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_stdout_ends_a_continuous_run() {
    let _serial = serial().await;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();
    // Like `echosrv-client --json -i 10ms | head -n 1`.
    let mut child = Command::new(BIN)
        .args(["tcp", &target, "--json", "-i", "10ms", "--rate", "100"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    stdout.read_line(&mut first).await.unwrap();
    assert!(first.starts_with(r#"{"type":"config""#), "{first}");
    drop(stdout);

    let status = tokio::time::timeout(RUN_TIMEOUT, child.wait())
        .await
        .expect("client kept running after stdout was closed")
        .unwrap();
    assert_eq!(status.code(), Some(141));
    server.stop().await;
}
