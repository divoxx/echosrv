//! Signal tests for the `echosrv-client` binary: the first SIGINT / SIGTERM
//! stops gracefully, a second one aborts.
//!
//! Like `tests/client_cli.rs`, the tests are serialized ([`serial`]). On
//! macOS, std sets `FD_CLOEXEC` on a new socket in a separate syscall, so a
//! child process spawned at that moment by another test can inherit the
//! socket and keep a port, connection or socket path alive after its owner
//! closed it.

mod client_common;
mod common;

use client_common::{RUN_TIMEOUT, client_command, describe, json_lines, send_signal};
use common::slow_echo_server;
use std::time::Duration;

/// The lock behind [`serial`]. A Tokio mutex, because the tests hold it
/// across `.await`.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes the tests in this file (see the module docs).
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}

/// The first `signal` stops gracefully with `reason`: the request in flight
/// is answered and counted.
async fn first_signal_lets_requests_in_flight_finish(signal: libc::c_int, reason: &str) {
    let _serial = serial().await;
    let delay = Duration::from_millis(300);
    let (addr, mut requests, server) = slow_echo_server(delay).await;
    let target = addr.to_string();
    let child = client_command(&["tcp", &target, "-c", "2", "-i", "0", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::timeout(common::WAIT, requests.recv())
        .await
        .expect("no request reached the server");

    let signalled = std::time::Instant::now();
    send_signal(&child, signal);
    let out = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
        .await
        .expect("client did not stop")
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let summary = json_lines(&out).pop().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["stop_reason"], reason);
    // The request in flight was answered and counted, not dropped.
    assert_eq!(summary["errors"], 0, "{}", describe(&out));
    assert!(summary["ok"].as_u64().unwrap() >= 1, "{}", describe(&out));
    assert!(
        signalled.elapsed() >= delay / 2,
        "stopped after {:?}, before the reply",
        signalled.elapsed()
    );
    server.abort();
}

#[tokio::test]
async fn sigterm_lets_requests_in_flight_finish() {
    first_signal_lets_requests_in_flight_finish(libc::SIGTERM, "terminated").await;
}

#[tokio::test]
async fn sigint_lets_requests_in_flight_finish() {
    first_signal_lets_requests_in_flight_finish(libc::SIGINT, "interrupt").await;
}

/// A second `signal` while a request waits for its reply aborts with `code`.
async fn second_signal_aborts(signal: libc::c_int, code: i32) {
    use tokio::io::AsyncBufReadExt;
    let _serial = serial().await;
    let (addr, mut requests, server) = slow_echo_server(Duration::from_secs(60)).await;
    let target = addr.to_string();
    let mut child = client_command(&["tcp", &target, "-i", "0", "-t", "120s"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    tokio::time::timeout(common::WAIT, requests.recv())
        .await
        .expect("no request reached the server");

    send_signal(&child, signal);
    let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    tokio::time::timeout(common::WAIT, async {
        while let Some(line) = stderr.next_line().await.unwrap() {
            if line.contains("stopping") {
                return;
            }
        }
        panic!("stderr closed before the stop notice");
    })
    .await
    .expect("first signal was not acknowledged");

    // The request is still waiting for its reply; a second signal aborts.
    send_signal(&child, signal);
    let status = tokio::time::timeout(common::WAIT, child.wait())
        .await
        .expect("second signal did not abort")
        .unwrap();
    assert_eq!(status.code(), Some(code));
    server.abort();
}

#[tokio::test]
async fn second_sigterm_aborts() {
    second_signal_aborts(libc::SIGTERM, 143).await;
}

#[tokio::test]
async fn second_sigint_aborts() {
    second_signal_aborts(libc::SIGINT, 130).await;
}

/// The handlers are in place once the header is out, before any request:
/// a signal at that moment still stops gracefully with a summary.
#[tokio::test]
async fn sigterm_right_after_header_still_reports() {
    use tokio::io::AsyncBufReadExt;
    let _serial = serial().await;
    let (addr, _requests, server) = slow_echo_server(Duration::from_millis(300)).await;
    let target = addr.to_string();
    let mut child = client_command(&["tcp", &target, "-c", "2", "-i", "0", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let header = tokio::time::timeout(common::WAIT, stdout.next_line())
        .await
        .expect("no header")
        .unwrap()
        .expect("stdout closed before the header");

    send_signal(&child, libc::SIGTERM);
    let rest = tokio::time::timeout(RUN_TIMEOUT, async {
        let mut lines = Vec::new();
        while let Some(line) = stdout.next_line().await.unwrap() {
            lines.push(line);
        }
        lines
    })
    .await
    .expect("client did not stop");
    let status = tokio::time::timeout(RUN_TIMEOUT, child.wait())
        .await
        .expect("client did not exit")
        .unwrap();
    assert_eq!(status.code(), Some(0), "header: {header}\nstdout: {rest:?}");
    let summary: serde_json::Value =
        serde_json::from_str(rest.last().expect("no summary")).unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["stop_reason"], "terminated");
    server.abort();
}
