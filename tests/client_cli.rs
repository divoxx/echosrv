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

mod client_common;
mod common;

use assert_cmd::Command as SyncCommand;
use client_common::{BIN, RUN_TIMEOUT, client_command, describe, json_lines, send_signal};
use common::{
    refused_addr, socket_dir, start_http, start_tcp, start_udp, start_unix_datagram_at,
    start_unix_stream_at,
};
use echosrv::http::HttpConfig;
use echosrv::{RateLimitConfig, TcpConfig, UdpConfig};
use predicates::prelude::*;
use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

/// A host name that fails to resolve without a DNS query: its first label is
/// 64 bytes, over the 63-byte limit.
const UNRESOLVABLE: &str =
    "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.invalid:80";
/// Bound for runs that only resolve a target.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

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

async fn run_client(args: &[&str]) -> Output {
    run_client_env(args, &[]).await
}

/// Runs the client with stdout/stderr piped, the color env vars cleared and
/// `env` applied on top.
async fn run_client_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = client_command(args);
    cmd.envs(env.iter().copied());
    tokio::time::timeout(RUN_TIMEOUT, cmd.output())
        .await
        .expect("echosrv-client timed out")
        .expect("failed to run echosrv-client")
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
        &["tcp", "nope"],
    ] {
        SyncCommand::new(BIN)
            .args(args)
            .env_remove("NO_COLOR")
            .assert()
            .code(2);
    }
    // An unresolvable host is a setup error. Its 64-byte label is over the DNS
    // limit of 63, so the resolver rejects it without a network lookup; the
    // timeout bounds the run if a resolver tries anyway.
    SyncCommand::new(BIN)
        .args(["tcp", UNRESOLVABLE, "-n", "1"])
        .timeout(RESOLVE_TIMEOUT)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot resolve"));
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

#[tokio::test(flavor = "multi_thread")]
async fn requests_and_duration_stop_at_whichever_comes_first() {
    let _serial = serial().await;
    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();

    // -n finishes long before -d.
    let started = std::time::Instant::now();
    let out = run_client(&["tcp", &target, "-n", "5", "-d", "30s", "--json"]).await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(started.elapsed() < Duration::from_secs(10));
    let summary = json_lines(&out).pop().unwrap();
    assert_eq!(summary["stop_reason"], "completed", "{summary}");
    assert_eq!(summary["ok"], 5, "{summary}");
    assert_eq!(summary["interrupted"], false, "{summary}");

    // -d ends a run that -n would keep going for minutes (shaped to 20/s).
    let out = run_client(&[
        "tcp", &target, "-n", "100000", "-d", "300ms", "--rate", "20", "--json",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let summary = json_lines(&out).pop().unwrap();
    assert_eq!(summary["stop_reason"], "duration", "{summary}");
    assert_eq!(summary["interrupted"], true, "{summary}");
    let ok = summary["ok"].as_u64().unwrap();
    assert!((1..100).contains(&ok), "{summary}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn conn_rate_paces_new_connections() {
    let _serial = serial().await;
    let server = start_tcp(TcpConfig::default()).await;
    let target = server.addr.to_string();
    // 10 connections at 20/s with a burst of 1: the last one opens >= 450ms
    // after the first.
    let out = run_client(&[
        "tcp",
        &target,
        "-n",
        "10",
        "--conn-mode",
        "per-request",
        "--conn-rate",
        "20",
        "-i",
        "0",
        "--json",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let lines = json_lines(&out);
    assert_eq!(lines[0]["conn_rate"], 20, "{}", lines[0]);
    let summary = lines.last().unwrap();
    assert_eq!(summary["ok"], 10, "{summary}");
    let elapsed = summary["elapsed_s"].as_f64().unwrap();
    assert!(elapsed >= 0.4, "10 connections at 20/s took {elapsed}s");
    server.stop().await;
}

/// Reads `lines` until one contains `needle` (bounded by [`common::WAIT`]),
/// keeping every line read.
async fn wait_for_line(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    seen: &mut Vec<String>,
    what: &str,
    pred: impl Fn(&str) -> bool,
) {
    tokio::time::timeout(common::WAIT, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            let hit = pred(&line);
            seen.push(line);
            if hit {
                return;
            }
        }
        panic!("stdout closed before {what}:\n{}", seen.join("\n"));
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}:\n{}", seen.join("\n")));
}

/// Stopping and restarting the server under a continuous run prints one
/// outage start line and one outage end line. Unix stream sockets keep the
/// run off the TCP port range.
#[tokio::test(flavor = "multi_thread")]
async fn live_outage_lines() {
    use tokio::io::AsyncBufReadExt;
    let _serial = serial().await;
    let dir = socket_dir();
    let path = dir.path().join("outage.sock");
    let server = start_unix_stream_at(&path).await;
    let mut child = client_command(&[
        "unix-stream",
        path.to_str().unwrap(),
        "--rate",
        "100",
        "-i",
        "100ms",
        "--reconnect-delay",
        "10ms",
        "--max-error-rate",
        "100",
    ])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null())
    .spawn()
    .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();

    wait_for_line(&mut lines, &mut seen, "traffic", |l| {
        l.contains(" ok=") && !l.contains(" ok=0 ")
    })
    .await;
    server.stop().await; // also removes the socket file
    wait_for_line(&mut lines, &mut seen, "outage start", |l| {
        l.contains("[fail] outage started (")
    })
    .await;
    // Stay down for a few intervals, so at least one is marked OUTAGE.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let server = start_unix_stream_at(&path).await;
    wait_for_line(&mut lines, &mut seen, "outage end", |l| {
        l.contains("[ok] outage ended after ")
    })
    .await;

    send_signal(&child, libc::SIGTERM);
    tokio::time::timeout(RUN_TIMEOUT, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            seen.push(line);
        }
    })
    .await
    .expect("client did not stop");
    let status = tokio::time::timeout(common::WAIT, child.wait())
        .await
        .expect("client did not exit")
        .unwrap();
    let stdout = seen.join("\n");
    assert_eq!(status.code(), Some(0), "{stdout}");
    assert!(
        seen.iter().any(|l| l.ends_with(" OUTAGE")),
        "no interval marked OUTAGE:\n{stdout}"
    );
    assert!(stdout.contains("outages     1 "), "{stdout}");
    server.stop().await;
}

/// Running out of local ports (`EADDRNOTAVAIL`) stops the run with its own
/// verdict and a hint on stderr. On macOS, connecting to port 0 fails with
/// `EADDRNOTAVAIL` without opening a connection.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn ports_exhausted_stops_with_its_verdict() {
    let _serial = serial().await;
    let out = run_client(&["tcp", "127.0.0.1:0", "-n", "5"]).await;
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ports_exhausted="), "{stdout}");
    assert!(
        stdout
            .trim_end()
            .ends_with("[fail] stopped: the client machine ran out of local ports"),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[fail] stopped: this machine ran out of local ports (EADDRNOTAVAIL)"),
        "{stderr}"
    );
    assert!(stderr.contains("lower --conn-rate or --rate"), "{stderr}");
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
