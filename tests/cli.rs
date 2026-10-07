//! Tests for the `echosrv` binary: argument handling, serving, signal-driven
//! shutdown and systemd-style socket activation.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_echosrv");
const WAIT: Duration = Duration::from_secs(10);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Serializes the tests in this file.
///
/// On macOS, std sets `FD_CLOEXEC` on new sockets with a separate syscall after
/// `socket()`, so a child forked concurrently by another test can inherit a
/// socket this test is about to close. The leaked copy keeps ports and socket
/// paths alive and makes readiness probes connect to the wrong process. Tests
/// that spawn processes therefore must not overlap with tests creating sockets.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Argument handling
// ---------------------------------------------------------------------------

#[test]
fn help_prints_usage_and_exits_zero() {
    let _serial = serial();
    for flag in ["--help", "-h"] {
        Command::new(BIN)
            .arg(flag)
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"))
            .stdout(predicate::str::contains("unix-stream"))
            .stdout(predicate::str::contains("--host"));
    }
}

#[test]
fn version_prints_crate_version() {
    let _serial = serial();
    for flag in ["--version", "-V"] {
        Command::new(BIN)
            .arg(flag)
            .assert()
            .success()
            .stdout(format!("echosrv {}\n", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn unknown_protocol_fails() {
    let _serial = serial();
    Command::new(BIN)
        .arg("gopher")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unknown protocol 'gopher'"));
}

#[test]
fn invalid_port_fails() {
    let _serial = serial();
    for port in ["notaport", "70000", "-1", ""] {
        Command::new(BIN)
            .args(["tcp", port])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("error:"));
    }
}

#[test]
fn unknown_option_and_bad_host_fail() {
    let _serial = serial();
    Command::new(BIN)
        .arg("--bogus")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unknown option"));
    Command::new(BIN)
        .args(["--host", "not-an-ip", "tcp"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("invalid host"));
    Command::new(BIN)
        .args(["--host", "127.0.0.1", "unix-stream"])
        .assert()
        .code(1);
}

#[test]
fn bind_failure_exits_nonzero() {
    let _serial = serial();
    // Port in use: the server must report the error and exit 1.
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    Command::new(BIN)
        .args(["tcp", &port])
        .timeout(WAIT)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Error"));
}

// ---------------------------------------------------------------------------
// Running servers
// ---------------------------------------------------------------------------

/// A running `echosrv` child process; killed on drop if still running.
struct Server {
    child: Child,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Server {
    fn spawn(args: &[&str]) -> Self {
        Self::spawn_with(std::process::Command::new(BIN).args(args))
    }

    /// Spawns `command` with stdout/stderr captured to a log file (so a chatty
    /// child can never block on a full pipe).
    fn spawn_with(command: &mut std::process::Command) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("server.log");
        let out = File::create(&log).unwrap();
        let child = command
            .env("RUST_LOG", "echosrv=debug")
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .expect("failed to spawn echosrv");
        Self {
            child,
            log,
            _dir: dir,
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Polls `probe` until it succeeds (the external process is ready),
    /// failing fast if the process exits.
    fn wait_until<T>(&mut self, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(value) = probe() {
                return value;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "echosrv exited early ({status}) waiting for {what}:\n{}",
                    self.log()
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends `signal` and waits for the process to exit.
    fn signal_and_wait(&mut self, signal: libc::c_int) -> ExitStatus {
        let pid = libc::pid_t::try_from(self.child.id()).unwrap();
        // SAFETY: `pid` is our own child, which has not been reaped yet.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0, "kill failed");
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "echosrv did not exit after signal {signal}:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends `signal` and asserts a clean (exit code 0) shutdown.
    fn stop_with(&mut self, signal: libc::c_int) {
        let status = self.signal_and_wait(signal);
        assert!(status.success(), "exit status {status}:\n{}", self.log());
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A loopback port that is free right now. The external process binds it a
/// moment later; that small window is acceptable for a CLI test.
fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn tcp_echo(addr: SocketAddr, msg: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.write_all(msg).unwrap();
    let mut buf = vec![0; msg.len()];
    stream.read_exact(&mut buf).unwrap();
    buf
}

fn socket_dir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("es").tempdir().unwrap()
}

#[test]
fn tcp_echoes_and_exits_cleanly_on_sigterm() {
    let _serial = serial();
    let addr: SocketAddr = ([127, 0, 0, 1], free_tcp_port()).into();
    let mut server = Server::spawn(&["tcp", &addr.port().to_string()]);
    server.wait_until("TCP listener", || TcpStream::connect(addr).ok());

    assert_eq!(tcp_echo(addr, b"hello cli"), b"hello cli");

    // A client still connected during shutdown must not prevent a clean exit.
    let idle = TcpStream::connect(addr).unwrap();
    server.stop_with(libc::SIGTERM);
    drop(idle);
    assert!(
        TcpStream::connect(addr).is_err(),
        "listener still open after exit"
    );
}

#[test]
fn tcp_honors_host_option_and_sigint() {
    let _serial = serial();
    let addr: SocketAddr = ([127, 0, 0, 1], free_tcp_port()).into();
    let mut server = Server::spawn(&["--host", "127.0.0.1", "tcp", &addr.port().to_string()]);
    server.wait_until("TCP listener", || TcpStream::connect(addr).ok());
    assert_eq!(tcp_echo(addr, b"via --host"), b"via --host");
    server.stop_with(libc::SIGINT);
}

#[test]
fn udp_echoes_and_exits_cleanly_on_sigterm() {
    let _serial = serial();
    let addr: SocketAddr = ([127, 0, 0, 1], free_udp_port()).into();
    let mut server = Server::spawn(&["udp", &addr.port().to_string()]);

    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut buf = [0u8; 64];
    let n = server.wait_until("UDP echo", || {
        client.send_to(b"udp ping", addr).ok()?;
        client.recv_from(&mut buf).ok().map(|(n, _)| n)
    });
    assert_eq!(&buf[..n], b"udp ping");

    server.stop_with(libc::SIGTERM);
}

#[test]
fn http_echoes_post_body() {
    let _serial = serial();
    let addr: SocketAddr = ([127, 0, 0, 1], free_tcp_port()).into();
    let mut server = Server::spawn(&["http", &addr.port().to_string()]);
    let mut stream = server.wait_until("HTTP listener", || TcpStream::connect(addr).ok());

    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream
        .write_all(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\nhello"), "{response}");

    server.stop_with(libc::SIGTERM);
}

fn wait_for_unix_stream(server: &mut Server, path: &Path) -> UnixStream {
    server.wait_until("Unix stream listener", || UnixStream::connect(path).ok())
}

#[test]
fn unix_stream_echoes_and_removes_socket_on_sigterm() {
    let _serial = serial();
    let dir = socket_dir();
    let path = dir.path().join("cli.sock");
    let mut server = Server::spawn(&["unix-stream", path.to_str().unwrap()]);

    let mut stream = wait_for_unix_stream(&mut server, &path);
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.write_all(b"unix cli").unwrap();
    let mut buf = [0u8; 8];
    stream.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"unix cli");
    drop(stream);

    server.stop_with(libc::SIGTERM);
    assert!(!path.exists(), "socket file left behind:\n{}", server.log());

    // Restarting on the same path works.
    let mut again = Server::spawn(&["unix-stream", path.to_str().unwrap()]);
    drop(wait_for_unix_stream(&mut again, &path));
    again.stop_with(libc::SIGINT);
    assert!(!path.exists());
}

#[test]
fn unix_stream_recovers_stale_socket_file() {
    let _serial = serial();
    let dir = socket_dir();
    let path = dir.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());

    let mut server = Server::spawn(&["unix-stream", path.to_str().unwrap()]);
    drop(wait_for_unix_stream(&mut server, &path));
    server.stop_with(libc::SIGTERM);
    assert!(!path.exists());
}

#[test]
fn unix_dgram_echoes_and_removes_socket_on_sigterm() {
    let _serial = serial();
    let dir = socket_dir();
    let path = dir.path().join("cli-dgram.sock");
    let mut server = Server::spawn(&["unix-dgram", path.to_str().unwrap()]);

    let client = UnixDatagram::bind(dir.path().join("client.sock")).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut buf = [0u8; 64];
    let n = server.wait_until("Unix datagram echo", || {
        client.send_to(b"dgram cli", &path).ok()?;
        client.recv(&mut buf).ok()
    });
    assert_eq!(&buf[..n], b"dgram cli");

    server.stop_with(libc::SIGTERM);
    assert!(!path.exists(), "socket file left behind:\n{}", server.log());
}

/// systemd-style socket activation: the parent creates the listener and the
/// child finds it as fd 3 with `LISTEN_FDS=1` and `LISTEN_PID=<its pid>`.
///
/// `LISTEN_PID` must equal the child's PID, which is only known after fork, so
/// a shell sets it to `$$` and then `exec`s echosrv (same PID).
#[test]
fn tcp_socket_activation_uses_inherited_listener() {
    let _serial = serial();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let inherited_addr = listener.local_addr().unwrap();
    let listener_fd = listener.as_raw_fd();
    // Tell the CLI to bind a different port; it must ignore it.
    let cli_port = free_tcp_port();
    assert_ne!(cli_port, inherited_addr.port());

    let mut command = std::process::Command::new("/bin/sh");
    command
        .args(["-c", r#"LISTEN_PID=$$ exec "$0" "$@""#, BIN, "tcp"])
        .arg(cli_port.to_string())
        .env("LISTEN_FDS", "1")
        .env_remove("LISTEN_PID")
        .env_remove("LISTEN_FDNAMES");
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if listener_fd == 3 {
                // Already in place; just clear close-on-exec.
                if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(listener_fd, 3) == -1 {
                // dup2 clears FD_CLOEXEC on the new descriptor.
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut server = Server::spawn_with(&mut command);
    // The child owns its copy now; closing ours means only the child can
    // answer on this port.
    drop(listener);

    let mut stream = server.wait_until("echo on inherited listener", || {
        let mut stream = TcpStream::connect(inherited_addr).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .ok()?;
        stream.write_all(b"activated").ok()?;
        let mut buf = [0u8; 9];
        stream.read_exact(&mut buf).ok()?;
        (&buf == b"activated").then_some(stream)
    });
    stream.write_all(b"again").unwrap();
    let mut buf = [0u8; 5];
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"again");
    drop(stream);

    let cli_addr: SocketAddr = ([127, 0, 0, 1], cli_port).into();
    assert!(
        TcpStream::connect(cli_addr).is_err(),
        "echosrv bound the CLI port instead of only using the inherited socket"
    );
    assert!(
        server.log().contains("inherited"),
        "expected an inheritance log line:\n{}",
        server.log()
    );

    server.stop_with(libc::SIGTERM);
}
