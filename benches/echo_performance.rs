use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use echosrv::{EchoClient, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::hint::black_box;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

/// A TCP echo server running in the background for the lifetime of a benchmark group.
struct BenchServer {
    addr: SocketAddr,
    shutdown: tokio::sync::broadcast::Sender<()>,
    handle: tokio::task::JoinHandle<echosrv::Result<()>>,
}

impl BenchServer {
    /// Starts a server on a free loopback port (port 0) and returns once it is listening.
    async fn start() -> Self {
        let config = TcpConfig {
            max_connections: 1000,
            buffer_size: 64 * 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            ..Default::default()
        };
        let server = TcpEchoServer::new(config.into());
        let shutdown = server.shutdown_signal();
        let bound = server
            .bind()
            .await
            .expect("failed to bind benchmark server");
        let addr = *bound
            .local_addr()
            .as_network()
            .expect("TCP server has a network address");
        let handle = tokio::spawn(bound.serve());
        Self {
            addr,
            shutdown,
            handle,
        }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = self.handle.await;
    }
}

/// Connects one client, outside any measured code.
///
/// Every benchmark reuses the clients it creates up front. Criterion runs a
/// routine tens of thousands of times, so a connection per iteration would
/// leave that many local ports in TIME_WAIT and could exhaust the ephemeral
/// port range. A whole `cargo bench` run opens a few dozen connections.
///
/// Clients are created right before the benchmark that uses them (not once
/// per group), so none sits idle long enough to hit the server's read
/// timeout while other benchmarks of the group run.
fn connect(rt: &Runtime, addr: SocketAddr) -> TcpEchoClient {
    rt.block_on(TcpEchoClient::connect(addr))
        .expect("failed to connect benchmark client")
}

fn bench_echo_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start());
    let addr = server.addr;

    let mut group = c.benchmark_group("echo_throughput");

    for size in [64usize, 256, 1024, 4096, 16384] {
        let mut client = connect(&rt, addr);
        let data = vec![b'x'; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("tcp_echo", size), &size, |b, _| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let start = Instant::now();
                    for _ in 0..iters {
                        let response = client.echo(black_box(&data)).await.unwrap();
                        assert_eq!(response.len(), data.len());
                        black_box(response);
                    }
                    start.elapsed()
                })
            });
        });
    }

    group.finish();
    rt.block_on(server.stop());
}

fn bench_concurrent_clients(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start());
    let addr = server.addr;

    let mut group = c.benchmark_group("concurrent_clients");

    for count in [1usize, 5, 10, 20] {
        let mut clients: Vec<TcpEchoClient> = (0..count).map(|_| connect(&rt, addr)).collect();
        let data = Arc::new(vec![b'x'; 1024]);
        // One iteration is `count` echoes, one per client, in parallel. Each
        // client runs on its own task for the whole sample and is handed back
        // for the next one.
        group.bench_with_input(
            BenchmarkId::new("concurrent_echo", count),
            &count,
            |b, _| {
                b.iter_custom(|iters| {
                    rt.block_on(async {
                        let start = Instant::now();
                        let handles: Vec<_> = clients
                            .drain(..)
                            .map(|mut client| {
                                let data = Arc::clone(&data);
                                tokio::spawn(async move {
                                    for _ in 0..iters {
                                        let response = client.echo(black_box(&data)).await.unwrap();
                                        assert_eq!(response.len(), data.len());
                                    }
                                    client
                                })
                            })
                            .collect();
                        for result in futures::future::join_all(handles).await {
                            clients.push(result.unwrap());
                        }
                        start.elapsed()
                    })
                });
            },
        );
    }

    group.finish();
    rt.block_on(server.stop());
}

fn bench_protocol_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start());
    let addr = server.addr;

    let mut group = c.benchmark_group("protocol_overhead");

    let mut client = connect(&rt, addr);
    group.bench_function("tcp_raw", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let start = Instant::now();
                for _ in 0..iters {
                    black_box(client.echo(black_box(b"Hello, World!")).await.unwrap());
                }
                start.elapsed()
            })
        });
    });

    group.finish();
    rt.block_on(server.stop());
}

criterion_group!(
    benches,
    bench_echo_throughput,
    bench_concurrent_clients,
    bench_protocol_overhead
);

criterion_main!(benches);
