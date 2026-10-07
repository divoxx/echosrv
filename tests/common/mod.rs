//! Shared helpers for integration tests.
//!
//! Every server is started with `bind()` (port `0` or a fresh temporary socket
//! path) *before* the helper returns, so the returned address is the real one
//! and already accepting: no readiness sleeps, no fixed ports and no
//! bind-drop-rebind races.
#![allow(dead_code)]

use echosrv::http::{HttpConfig, HttpEchoServer};
use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
use echosrv::{
    EchoServerTrait, Result, TcpConfig, TcpEchoServer, UdpConfig, UdpEchoServer,
    UnixDatagramEchoServer, UnixStreamEchoServer,
};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Upper bound for anything a test waits on; generous so slow CI does not
/// flake, but finite so a hang fails instead of stalling the suite.
pub const WAIT: Duration = Duration::from_secs(5);

/// A server serving in a background task.
pub struct TestServer<A> {
    /// The address the server is actually bound to.
    pub addr: A,
    shutdown: broadcast::Sender<()>,
    handle: JoinHandle<Result<()>>,
}

impl<A> TestServer<A> {
    fn spawn(
        addr: A,
        shutdown: broadcast::Sender<()>,
        serve: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Self {
        Self {
            addr,
            shutdown,
            handle: tokio::spawn(serve),
        }
    }

    /// A sender that triggers a graceful shutdown.
    pub fn shutdown_sender(&self) -> broadcast::Sender<()> {
        self.shutdown.clone()
    }

    /// Requests a graceful shutdown and asserts that `serve()` returns `Ok(())`
    /// within [`WAIT`]. Returns the server address.
    pub async fn stop(self) -> A {
        self.shutdown
            .send(())
            .expect("server dropped its shutdown receiver before stop()");
        self.join().await
    }

    /// Waits for the server task (after shutdown was requested elsewhere) and
    /// asserts it returned `Ok(())`.
    pub async fn join(self) -> A {
        tokio::time::timeout(WAIT, self.handle)
            .await
            .expect("server did not stop within the timeout")
            .expect("server task panicked")
            .expect("serve() returned an error");
        self.addr
    }
}

fn network_addr(addr: &echosrv::Address) -> SocketAddr {
    *addr
        .as_network()
        .expect("server bound to a non-network address")
}

fn unix_path(addr: &echosrv::Address) -> PathBuf {
    addr.as_unix()
        .expect("server bound to a non-Unix address")
        .clone()
}

/// Starts a TCP echo server. `bind_addr` defaults to `127.0.0.1:0`.
pub async fn start_tcp(config: TcpConfig) -> TestServer<SocketAddr> {
    let server = TcpEchoServer::new(config.into());
    let bound = server.bind().await.expect("failed to bind TCP server");
    let addr = network_addr(bound.local_addr());
    TestServer::spawn(addr, server.shutdown_signal(), bound.serve())
}

/// Starts a TCP echo server with default settings and the given connection limit.
pub async fn start_tcp_with_limit(max_connections: usize) -> TestServer<SocketAddr> {
    start_tcp(TcpConfig {
        max_connections,
        ..Default::default()
    })
    .await
}

/// Starts a UDP echo server. `bind_addr` defaults to `127.0.0.1:0`.
pub async fn start_udp(config: UdpConfig) -> TestServer<SocketAddr> {
    try_start_udp(config)
        .await
        .expect("failed to bind UDP server")
}

/// Like [`start_udp`] but returns the bind error (e.g. IPv6 unavailable).
pub async fn try_start_udp(config: UdpConfig) -> Result<TestServer<SocketAddr>> {
    let server = UdpEchoServer::new(config.into());
    let bound = server.bind().await?;
    let addr = network_addr(bound.local_addr());
    Ok(TestServer::spawn(
        addr,
        server.shutdown_signal(),
        bound.serve(),
    ))
}

/// Starts an HTTP echo server. `bind_addr` defaults to `127.0.0.1:0`.
pub async fn start_http(config: HttpConfig) -> TestServer<SocketAddr> {
    let server = HttpEchoServer::new(config);
    let bound = server.bind().await.expect("failed to bind HTTP server");
    let addr = network_addr(bound.local_addr());
    TestServer::spawn(addr, server.shutdown_signal(), bound.serve())
}

/// Starts a Unix stream echo server.
pub async fn start_unix_stream(config: UnixStreamConfig) -> TestServer<PathBuf> {
    try_start_unix_stream(config)
        .await
        .expect("failed to bind Unix stream server")
}

/// Like [`start_unix_stream`] but returns the bind error.
pub async fn try_start_unix_stream(config: UnixStreamConfig) -> Result<TestServer<PathBuf>> {
    let server = UnixStreamEchoServer::new(config);
    let bound = server.bind().await?;
    let path = unix_path(bound.local_addr());
    Ok(TestServer::spawn(
        path,
        server.shutdown_signal(),
        bound.serve(),
    ))
}

/// Starts a Unix stream echo server on `path` with default settings.
pub async fn start_unix_stream_at(path: &Path) -> TestServer<PathBuf> {
    start_unix_stream(UnixStreamConfig::default().with_socket_path(path.to_path_buf())).await
}

/// Starts a Unix datagram echo server.
pub async fn start_unix_datagram(config: UnixDatagramConfig) -> TestServer<PathBuf> {
    try_start_unix_datagram(config)
        .await
        .expect("failed to bind Unix datagram server")
}

/// Like [`start_unix_datagram`] but returns the bind error.
pub async fn try_start_unix_datagram(config: UnixDatagramConfig) -> Result<TestServer<PathBuf>> {
    let server = UnixDatagramEchoServer::new(config);
    let bound = server.bind().await?;
    let path = unix_path(bound.local_addr());
    Ok(TestServer::spawn(
        path,
        server.shutdown_signal(),
        bound.serve(),
    ))
}

/// Starts a Unix datagram echo server on `path` with default settings.
pub async fn start_unix_datagram_at(path: &Path) -> TestServer<PathBuf> {
    start_unix_datagram(UnixDatagramConfig::default().with_socket_path(path.to_path_buf())).await
}

/// A fresh temporary directory for socket files. Kept short because Unix
/// socket paths are limited to ~104 bytes on macOS.
pub fn socket_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("es")
        .tempdir()
        .expect("failed to create temp dir")
}

/// Deterministic, non-repeating-looking binary payload of `len` bytes
/// (includes NUL bytes).
pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// A payload unique to `id`, so concurrent clients can detect cross-talk.
pub fn tagged_payload(id: usize, len: usize) -> Vec<u8> {
    let mut data = format!("client-{id}:").into_bytes();
    data.extend((0..len).map(|i| ((i + id * 7) % 256) as u8));
    data
}
