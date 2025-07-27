# Development Guide

This document provides comprehensive guidance for developers working with the EchoSrv codebase, covering architecture, development practices, and contribution guidelines.

## Architecture Overview

### System Design

EchoSrv is a high-performance async echo server library built with Tokio that supports multiple protocols through a clean, generic architecture:

1. **Generic Protocol System**: Abstract protocol implementations through traits
2. **Type Aliases**: Concrete protocol types are aliases over generic components
3. **Unified Address System**: Single address type supporting network and Unix sockets
4. **Resource Management**: Built-in security, rate limiting, and performance optimizations
5. **Configuration System**: Fluent builder pattern for type-safe configuration

### Module Structure

```
src/
├── lib.rs              # Library entry point and error types
├── main.rs             # Binary entry point for standalone server
├── common/             # Shared traits and utilities
│   ├── traits.rs       # Core traits (EchoServerTrait, EchoClient)
│   └── test_utils.rs   # Test utilities and helper functions
├── network/            # Unified addressing and configuration
│   ├── address.rs      # Address enum (Network/Unix)
│   └── config.rs       # Configuration builders and types
├── security/           # Resource limits and protection
│   └── limits.rs       # Rate limiting, connection tracking, size validation
├── performance/        # Performance optimizations
│   └── buffer_pool.rs  # Buffer pooling and memory management
├── stream/             # Generic stream implementation
│   ├── server.rs       # StreamEchoServer<P> generic server
│   ├── client.rs       # StreamEchoClient<P> generic client
│   ├── protocol.rs     # StreamProtocol trait definition
│   └── config.rs       # Stream-specific configuration
├── datagram/           # Generic datagram implementation
│   ├── server.rs       # DatagramEchoServer<P> generic server
│   ├── client.rs       # DatagramEchoClient<P> generic client
│   ├── protocol.rs     # DatagramProtocol trait definition
│   └── config.rs       # Datagram-specific configuration
├── tcp/                # TCP protocol implementation
│   ├── mod.rs          # Type aliases and exports
│   ├── config.rs       # TcpConfig
│   └── stream_protocol.rs # TcpProtocol implementation
├── udp/                # UDP protocol implementation
│   ├── mod.rs          # Type aliases and exports
│   ├── config.rs       # UdpConfig
│   └── datagram_protocol.rs # UdpProtocol implementation
├── unix/               # Unix domain socket implementation
│   ├── mod.rs          # Type aliases and exports
│   ├── config.rs       # Unix socket configurations
│   ├── server.rs       # Unix-specific server implementations
│   ├── client.rs       # Unix-specific client implementations
│   ├── stream_protocol.rs # Unix stream protocol
│   └── datagram_protocol.rs # Unix datagram protocol
└── http/               # HTTP protocol implementation
    ├── mod.rs          # Type aliases and exports
    ├── config.rs       # HttpConfig
    ├── protocol.rs     # HttpProtocol implementation
    └── client.rs       # HttpEchoClient type alias
```

### Core Concepts

#### Unified Address System

All protocols use the `Address` enum for addressing:

```rust
pub enum Address {
    Network(SocketAddr),  // TCP, UDP, HTTP
    Unix(PathBuf),        // Unix domain sockets
}

// Usage examples
let tcp_addr: Address = "127.0.0.1:8080".parse()?;
let unix_addr: Address = "unix:/tmp/echo.sock".into();
```

#### Generic Protocol Architecture

Concrete protocol types are type aliases over generic implementations:

```rust
// Type aliases provide familiar APIs
pub type TcpEchoServer = StreamEchoServer<TcpProtocol>;
pub type TcpEchoClient = StreamEchoClient<TcpProtocol>;
pub type UdpEchoServer = DatagramEchoServer<UdpProtocol>;
pub type UdpEchoClient = DatagramEchoClient<UdpProtocol>;

// All share the same generic implementation
let tcp_server = TcpEchoServer::new(config);
let udp_server = UdpEchoServer::new(config);
```

#### Configuration System

Fluent builder pattern for type-safe configuration:

```rust
let config = TcpConfig {
    bind_addr: "127.0.0.1:8080".parse()?,
    max_connections: 100,
    buffer_size: 8192,
    read_timeout: Duration::from_secs(30),
    write_timeout: Duration::from_secs(30),
};

// Or use builders for network configuration
let config = Config::new("127.0.0.1:8080".into())
    .with_buffer_size(4096)
    .with_read_timeout(Duration::from_secs(60))
    .build();
```

#### Resource Management

Built-in protection against resource exhaustion:

```rust
// Rate limiting
let limiter = RateLimiter::new(100); // 100 requests per second

// Connection tracking
let limits = ResourceLimits {
    max_connections: 1000,
    max_connections_per_ip: 10,
    connection_timeout: Duration::from_secs(300),
};

// Size validation
let validator = SizeValidator::new(1024 * 1024); // 1MB max
```

#### Performance Optimizations

Buffer pooling reduces allocations:

```rust
// Global buffer pool
let pool = global_pool();
let buffer = pool.get(); // Reusable buffer

// Pool statistics
let stats = pool.stats();
println!("Pool hits: {}, misses: {}", stats.hits, stats.misses);
```

## Development Setup

### Prerequisites

- Rust 1.70+ (for async traits)
- Cargo for build management
- Git for version control

### Quick Start

```bash
# Clone and build
git clone <repository-url>
cd echosrv
cargo build

# Run tests
cargo test

# Run specific protocol tests
cargo test tcp
cargo test udp
cargo test http
cargo test unix

# Code quality checks
cargo clippy
cargo fmt

# Run examples
cargo run tcp          # TCP server on port 8080
cargo run udp 9090     # UDP server on specific port
cargo run http         # HTTP server on port 8080
cargo run unix-stream /tmp/echo.sock
```

### Development Workflow

1. **Feature Development**:
   ```bash
   git checkout -b feature/new-feature
   # Make changes
   cargo test
   cargo clippy
   cargo fmt
   git commit -m "feat: add new feature"
   ```

2. **Testing Strategy**:
   - Unit tests for individual components
   - Integration tests for cross-component interaction
   - Property-based tests for data validation
   - Performance benchmarks for regression detection

## Core APIs

### Server Usage

```rust
use echosrv::{TcpEchoServer, TcpConfig, EchoServerTrait};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = TcpConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        max_connections: 100,
        buffer_size: 4096,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };
    
    let server = TcpEchoServer::new(config.into());
    server.run().await?;
    Ok(())
}
```

### Client Usage

```rust
use echosrv::{TcpEchoClient, EchoClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = TcpEchoClient::connect("127.0.0.1:8080").await?;
    
    let response = client.echo_string("Hello, World!").await?;
    println!("Response: {}", response);
    
    Ok(())
}
```

### Error Handling

The library uses structured error types:

```rust
use echosrv::{Result, EchoError};

fn example() -> Result<()> {
    // Library functions return Result<T, EchoError>
    match some_operation() {
        Ok(value) => println!("Success: {:?}", value),
        Err(EchoError::Tcp(io_err)) => eprintln!("TCP error: {}", io_err),
        Err(EchoError::Timeout(msg)) => eprintln!("Timeout: {}", msg),
        Err(EchoError::Config(msg)) => eprintln!("Configuration error: {}", msg),
        Err(err) => eprintln!("Other error: {}", err),
    }
    Ok(())
}
```

## Protocol Development

### Adding a New Stream Protocol

1. **Implement the StreamProtocol trait**:
```rust
pub struct MyStreamProtocol;

#[async_trait]
impl StreamProtocol for MyStreamProtocol {
    type Error = MyError;
    type Listener = MyListener;
    type Stream = MyStream;
    
    async fn connect(addr: SocketAddr) -> Result<Self::Stream, Self::Error> {
        // Connect implementation
    }
    
    async fn bind(config: &StreamConfig) -> Result<Self::Listener, Self::Error> {
        // Bind implementation
    }
    
    // ... other required methods
}
```

2. **Create type aliases**:
```rust
pub type MyEchoServer = StreamEchoServer<MyStreamProtocol>;
pub type MyEchoClient = StreamEchoClient<MyStreamProtocol>;
```

3. **Add configuration**:
```rust
#[derive(Debug, Clone)]
pub struct MyConfig {
    pub bind_addr: SocketAddr,
    pub custom_option: String,
    // ... other protocol-specific options
}
```

### Adding a New Datagram Protocol

Similar process but implement `DatagramProtocol` trait instead.

## Testing Guidelines

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_component_functionality() {
        // Test individual component behavior
        let component = MyComponent::new();
        let result = component.process("test").await;
        assert_eq!(result.unwrap(), "expected");
    }
    
    #[test]
    fn test_configuration_validation() {
        // Test configuration validation
        let config = MyConfig::default();
        assert!(config.validate().is_ok());
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_end_to_end_communication() -> Result<()> {
    // Start server
    let (server_handle, addr) = create_test_server().await?;
    
    // Test client communication
    let mut client = TcpEchoClient::connect(addr).await?;
    let response = client.echo_string("test message").await?;
    assert_eq!(response, "test message");
    
    // Cleanup
    server_handle.abort();
    Ok(())
}
```

### Property-Based Tests

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn echo_preserves_data(data in prop::collection::vec(any::<u8>(), 0..1024)) {
        tokio_test::block_on(async {
            let (server_handle, addr) = create_test_server().await.unwrap();
            let mut client = TcpEchoClient::connect(addr).await.unwrap();
            let response = client.echo(&data).await.unwrap();
            assert_eq!(response, data);
            server_handle.abort();
        });
    }
}
```

## Performance Considerations

### Memory Management

- **Buffer Pooling**: Use `global_pool()` for reusable buffers
- **Size Limits**: Configure appropriate buffer and message sizes
- **Zero-Copy**: Leverage `Bytes` type where possible

### Concurrency

- **Connection Limits**: Set appropriate `max_connections`
- **Timeout Configuration**: Balance responsiveness vs resource usage
- **Resource Tracking**: Monitor connection and buffer pool metrics

### Protocol-Specific Optimizations

- **Stream Protocols**: Connection pooling, keep-alive
- **Datagram Protocols**: Batch processing, stateless scaling
- **HTTP**: Request pipelining, header optimization

## Security Guidelines

### Input Validation

```rust
// Validate configuration
if config.buffer_size > MAX_BUFFER_SIZE {
    return Err(EchoError::Config("Buffer size too large".to_string()));
}

// Validate request sizes
let validator = SizeValidator::new(config.max_request_size);
validator.validate(&request_data)?;
```

### Resource Protection

```rust
// Rate limiting
let limiter = RateLimiter::new(config.rate_limit);
limiter.acquire().await?;

// Connection tracking
let tracker = ConnectionTracker::new(limits);
let _guard = tracker.acquire_connection(client_addr).await?;
```

### Error Handling

```rust
// Don't leak sensitive information
match internal_operation() {
    Ok(result) => Ok(result),
    Err(_) => Err(EchoError::Config("Operation failed".to_string())),
}
```

## Debugging and Monitoring

### Logging

```rust
use tracing::{info, debug, error, instrument};

#[instrument]
async fn handle_connection(stream: TcpStream, addr: SocketAddr) -> Result<()> {
    info!("New connection from {}", addr);
    
    match process_request(&stream).await {
        Ok(()) => debug!("Request processed successfully"),
        Err(e) => error!("Request failed: {}", e),
    }
    
    Ok(())
}
```

### Metrics

```rust
// Connection metrics
let stats = connection_tracker.stats();
println!("Active connections: {}", stats.active);

// Buffer pool metrics
let pool_stats = global_pool().stats();
println!("Pool efficiency: {:.2}%", 
         pool_stats.hits as f64 / (pool_stats.hits + pool_stats.misses) as f64 * 100.0);

// Rate limiting metrics
let rate_stats = rate_limiter.stats();
println!("Requests allowed: {}, denied: {}", rate_stats.allowed, rate_stats.denied);
```

### Performance Profiling

```bash
# Run benchmarks
cargo bench

# Profile with perf
perf record --call-graph=dwarf cargo test --release
perf report

# Memory profiling with valgrind
valgrind --tool=massif cargo test --release

# Async runtime debugging
TOKIO_CONSOLE_BIND=127.0.0.1:6669 cargo run --features tokio-console
```

## Code Style and Standards

### Documentation

```rust
/// Handles client connections with configurable timeouts and resource limits.
///
/// This function processes incoming client connections, applying rate limiting,
/// connection tracking, and size validation before echoing data back.
///
/// # Arguments
///
/// * `stream` - The incoming TCP stream
/// * `config` - Server configuration including timeouts and limits
///
/// # Returns
///
/// Returns `Ok(())` on successful completion or an `EchoError` on failure.
///
/// # Examples
///
/// ```no_run
/// use echosrv::{TcpConfig, handle_connection};
///
/// let config = TcpConfig::default();
/// handle_connection(stream, config).await?;
/// ```
///
/// # Errors
///
/// * `EchoError::Timeout` - Operation exceeded configured timeout
/// * `EchoError::RateLimit` - Client exceeded rate limit
/// * `EchoError::Tcp` - Underlying I/O error
pub async fn handle_connection(stream: TcpStream, config: &TcpConfig) -> Result<()> {
    // Implementation
}
```

### Error Patterns

```rust
// Good: Structured errors with context
return Err(EchoError::Config(format!(
    "Invalid buffer size: {} (max: {})", 
    size, MAX_BUFFER_SIZE
)));

// Good: Convert underlying errors
stream.read(&mut buffer).await
    .map_err(EchoError::Tcp)?;

// Avoid: Generic error messages
return Err(EchoError::Config("Invalid configuration".to_string()));
```

### Testing Patterns

```rust
// Good: Descriptive test names
#[tokio::test]
async fn tcp_server_rejects_connections_when_limit_exceeded() {
    // Test implementation
}

// Good: Proper cleanup
#[tokio::test]
async fn test_with_cleanup() -> Result<()> {
    let (server_handle, _addr) = create_test_server().await?;
    
    // Test logic here
    
    server_handle.abort(); // Always cleanup
    Ok(())
}
```

## Contributing

### Pull Request Process

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/amazing-feature`
3. Make changes following the style guide
4. Add comprehensive tests
5. Update documentation as needed
6. Run the full test suite: `cargo test`
7. Submit a pull request with clear description

### Commit Message Format

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`
Scopes: `tcp`, `udp`, `http`, `unix`, `stream`, `datagram`, `security`, `perf`

### Code Review Guidelines

- Verify correctness and test coverage
- Check for security implications
- Review performance impact
- Ensure proper error handling
- Validate documentation updates

## Troubleshooting

### Common Issues

**Connection Refused**: Server not running or port unavailable
```bash
netstat -tlnp | grep 8080  # Check if port is in use
```

**Test Failures**: Port conflicts or resource cleanup issues
```bash
cargo test -- --test-threads=1  # Run tests sequentially
```

**Memory Issues**: Check connection limits and buffer pool configuration
```rust
let stats = global_pool().stats();
println!("Pool stats: {:?}", stats);
```

**Performance Issues**: Profile and check resource utilization
```bash
cargo bench  # Run performance benchmarks
```

### Debug Logging

```bash
# Enable detailed logging
RUST_LOG=debug cargo test -- --nocapture

# Protocol-specific logging
RUST_LOG=echosrv::tcp=trace cargo run tcp

# Async runtime debugging
RUST_LOG=tokio=debug cargo test
```

This guide provides the foundation for understanding and contributing to EchoSrv. The architecture is designed for extensibility while maintaining performance and security, making it suitable for both development testing and production deployment scenarios.