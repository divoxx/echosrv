use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use echosrv::{EchoClient, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::hint::black_box;
use std::net::SocketAddr;
use std::time::Duration;
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

fn bench_echo_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start());
    let addr = server.addr;

    let mut group = c.benchmark_group("echo_throughput");

    for size in [64usize, 256, 1024, 4096, 16384] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("tcp_echo", size), &size, |b, &size| {
            let data = vec![b'x'; size];
            b.to_async(&rt).iter(|| async {
                let mut client = TcpEchoClient::connect(addr).await.unwrap();
                let response = client.echo(black_box(&data)).await.unwrap();
                assert_eq!(response.len(), data.len());
                response
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
        group.bench_with_input(
            BenchmarkId::new("concurrent_echo", count),
            &count,
            |b, &count| {
                let data = vec![b'x'; 1024];
                b.to_async(&rt).iter(|| async {
                    let handles: Vec<_> = (0..count)
                        .map(|_| {
                            let data = data.clone();
                            tokio::spawn(async move {
                                let mut client = TcpEchoClient::connect(addr).await.unwrap();
                                client.echo(black_box(&data)).await.unwrap()
                            })
                        })
                        .collect();

                    let results = futures::future::join_all(handles).await;
                    for result in results {
                        assert_eq!(result.unwrap().len(), data.len());
                    }
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

    group.bench_function("tcp_raw", |b| {
        b.to_async(&rt).iter(|| async {
            let mut client = TcpEchoClient::connect(addr).await.unwrap();
            client.echo(black_box(b"Hello, World!")).await.unwrap()
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
