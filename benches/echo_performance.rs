use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use echosrv::{TcpEchoServer, TcpEchoClient, TcpConfig, EchoClient, EchoServerTrait};
use echosrv::performance::{BufferPool, global_pool};
use std::time::Duration;
use tokio::runtime::Runtime;

fn bench_echo_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("echo_throughput");
    
    // Test different message sizes
    let sizes = vec![64, 256, 1024, 4096, 16384];
    
    for size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("tcp_echo", size),
            &size,
            |b, &size| {
                b.to_async(&rt).iter(|| async {
                    // Setup server
                    let config = TcpConfig {
                        bind_addr: "127.0.0.1:0".parse().unwrap(),
                        max_connections: 100,
                        buffer_size: 8192,
                        read_timeout: Duration::from_secs(30),
                        write_timeout: Duration::from_secs(30),
                    };
                    
                    let server = TcpEchoServer::new(config.clone().into());
                    let addr = config.bind_addr;
                    
                    let server_handle = tokio::spawn(async move {
                        server.run().await
                    });
                    
                    // Give server time to start
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    
                    // Connect client
                    let mut client = TcpEchoClient::connect(addr).await.unwrap();
                    
                    // Create test data
                    let data = vec![b'x'; size];
                    
                    // Benchmark the echo operation
                    let response = client.echo(black_box(&data)).await.unwrap();
                    assert_eq!(response.len(), data.len());
                    
                    server_handle.abort();
                    response
                });
            },
        );
    }
    
    group.finish();
}

fn bench_concurrent_clients(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("concurrent_clients");
    
    // Test different numbers of concurrent clients
    let client_counts = vec![1, 5, 10, 20];
    
    for count in client_counts {
        group.bench_with_input(
            BenchmarkId::new("concurrent_echo", count),
            &count,
            |b, &count| {
                b.to_async(&rt).iter(|| async {
                    // Setup server
                    let config = TcpConfig {
                        bind_addr: "127.0.0.1:0".parse().unwrap(),
                        max_connections: 100,
                        buffer_size: 8192,
                        read_timeout: Duration::from_secs(30),
                        write_timeout: Duration::from_secs(30),
                    };
                    
                    let server = TcpEchoServer::new(config.clone().into());
                    let addr = config.bind_addr;
                    
                    let server_handle = tokio::spawn(async move {
                        server.run().await
                    });
                    
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    
                    // Create concurrent client tasks
                    let mut handles = Vec::new();
                    let data = vec![b'x'; 1024];
                    
                    for _ in 0..count {
                        let addr = addr;
                        let data = data.clone();
                        let handle = tokio::spawn(async move {
                            let mut client = TcpEchoClient::connect(addr).await.unwrap();
                            client.echo(black_box(&data)).await.unwrap()
                        });
                        handles.push(handle);
                    }
                    
                    // Wait for all clients to complete
                    let results = futures::future::join_all(handles).await;
                    
                    server_handle.abort();
                    
                    // Verify all responses
                    for result in results {
                        let response = result.unwrap();
                        assert_eq!(response.len(), data.len());
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn bench_buffer_pool(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_pool");
    
    // Compare buffer allocation strategies
    group.bench_function("vec_allocation", |b| {
        b.iter(|| {
            let mut buffer = Vec::with_capacity(black_box(8192));
            buffer.extend_from_slice(&vec![b'x'; 1024]);
            buffer.clear();
            buffer
        });
    });
    
    group.bench_function("pooled_buffer", |b| {
        let pool = BufferPool::new(8192, 100);
        b.iter(|| {
            let mut buffer = pool.get();
            buffer.extend_from_slice(&vec![b'x'; 1024]);
            buffer.clear();
            buffer
        });
    });
    
    group.bench_function("global_pool", |b| {
        b.iter(|| {
            let mut buffer = global_pool().get();
            buffer.extend_from_slice(&vec![b'x'; 1024]);
            buffer.clear();
            buffer
        });
    });
    
    group.finish();
}

fn bench_protocol_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("protocol_overhead");
    
    // Measure raw TCP vs HTTP overhead (when HTTP is implemented)
    group.bench_function("tcp_raw", |b| {
        b.to_async(&rt).iter(|| async {
            let config = TcpConfig {
                bind_addr: "127.0.0.1:0".parse().unwrap(),
                max_connections: 100,
                buffer_size: 8192,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
            };
            
            let server = TcpEchoServer::new(config.clone().into());
            let addr = config.bind_addr;
            
            let server_handle = tokio::spawn(async move {
                server.run().await
            });
            
            tokio::time::sleep(Duration::from_millis(10)).await;
            
            let mut client = TcpEchoClient::connect(addr).await.unwrap();
            let data = b"Hello, World!";
            
            let response = client.echo(black_box(data)).await.unwrap();
            
            server_handle.abort();
            response
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_echo_throughput,
    bench_concurrent_clients,
    bench_buffer_pool,
    bench_protocol_overhead
);

criterion_main!(benches);