# Development Guide

This document outlines the development practices, architecture decisions, and guidelines for contributing to the Echo Server project.

## Architecture Overview

### System Design

The Echo Server is built around a clean, generic, multi-protocol architecture:

1. **Generic Interface**: Shared traits and utilities for both stream and datagram protocols
2. **Protocol Trait System**: `StreamProtocol` and `DatagramProtocol` traits define protocol interfaces
3. **Generic Implementations**: `StreamEchoServer/Client` and `DatagramEchoServer/Client` work with any protocol
4. **Type Aliases**: Concrete TCP/UDP clients are type aliases to generic implementations
5. **Configuration Management**: Protocol-specific configuration structs
6. **Connection Handling**: Async tasks handle connections/datagrams per protocol

### Module Structure

```
src/
├── lib.rs              # Library entry point, exports all modules
├── main.rs             # Binary entry point for standalone server
├── common/             # Shared components
│   ├── mod.rs          # Common module exports
│   ├── traits.rs       # Core traits (EchoServerTrait, EchoClient)
│   └── test_utils.rs   # Test utilities
├── stream/             # Generic stream implementation
│   ├── mod.rs          # Stream module exports
│   ├── server.rs       # Generic stream server (StreamEchoServer<P>)
│   ├── client.rs       # Generic stream client (StreamEchoClient<P>)
│   ├── protocol.rs     # StreamProtocol trait
│   └── config.rs       # StreamConfig
├── datagram/           # Generic datagram implementation
│   ├── mod.rs          # Datagram module exports
│   ├── server.rs       # Generic datagram server (DatagramEchoServer<P>)
│   ├── client.rs       # Generic datagram client (DatagramEchoClient<P>)
│   ├── protocol.rs     # DatagramProtocol trait
│   └── config.rs       # DatagramConfig
├── tcp/                # TCP-specific implementation
│   ├── mod.rs          # Type aliases: TcpEchoClient = StreamEchoClient<TcpProtocol>
│   ├── server.rs       # Type alias: TcpEchoServer = StreamEchoServer<TcpProtocol>
│   ├── config.rs       # TcpConfig
│   ├── stream_protocol.rs # TcpProtocol implementation
│   └── tests.rs        # TCP-specific tests
└── udp/                # UDP-specific implementation
    ├── mod.rs          # Type aliases: UdpEchoClient = DatagramEchoClient<UdpProtocol>
    ├── server.rs       # Type alias: UdpEchoServer = DatagramEchoServer<UdpProtocol>
    ├── config.rs       # UdpConfig
    ├── datagram_protocol.rs # UdpProtocol implementation
    └── tests.rs        # UDP-specific tests
```

### Key Design Decisions

#### 1. Generic Architecture
- **Implementation**: Generic `StreamEchoServer<P>` and `DatagramEchoServer<P>` over protocol traits
- **Benefits**: Code reuse, extensibility, consistent API across protocols
- **Design**: Protocol traits (`StreamProtocol`, `DatagramProtocol`) define the interface

#### 2. Type Alias Pattern
- **Implementation**: Concrete clients are type aliases to generic implementations
- **Benefits**: Zero-cost abstraction, familiar API, extensibility
- **Example**: `pub type TcpEchoClient = StreamEchoClient<TcpProtocol>`

#### 3. Protocol Trait System
- **StreamProtocol**: Defines interface for stream-based protocols (TCP, Unix streams, WebSockets)
- **DatagramProtocol**: Defines interface for datagram-based protocols (UDP, Unix datagrams)
- **Benefits**: Easy to add new protocols, consistent interface, compile-time safety

#### 4. Async Architecture
- **Implementation**: Uses Tokio runtime for async/await support
- **Benefits**: Efficient resource utilization, high concurrency

#### 5. Error Handling Architecture
- **Library**: Uses `thiserror` for structured error types
- **Binary**: Uses `color-eyre` for user-friendly error reporting
- **Benefits**: Library independence, composable errors, better debugging experience

#### 6. Logging
- **Implementation**: Uses `tracing` for structured logging
- **Benefits**: Configurable log levels, structured data, performance

#### 7. Connection Management
- **TCP**: Atomic connection counting with configurable limits
- **UDP**: Stateless datagram handling
- **Benefits**: Predictable resource usage, graceful degradation

#### 8. Timeout Configuration
- **Implementation**: Configurable read/write timeouts per protocol
- **Benefits**: Automatic cleanup, predictable behavior

## Development Setup

### Prerequisites

- Rust 1.70+ (for async traits in traits)
- Cargo
- Git

### Local Development

```bash
# Clone the repository
git clone <repository-url>
cd echosrv

# Build the project
cargo build

# Run tests
cargo test

# Run with logging
RUST_LOG=info cargo test -- --nocapture

# Test specific protocol
cargo test --test integration_tests test_tcp
cargo test --test integration_tests test_udp
```

### Development Workflow

1. **Feature Development**:
   ```bash
   # Create feature branch
   git checkout -b feature/new-feature
   
   # Make changes
   # Run tests
   cargo test
   
   # Check code quality
   cargo clippy
   cargo fmt
   
   # Commit changes
   git commit -m "feat: add new feature"
   ```

2. **Testing Strategy**:
   - Unit tests for individual functions
   - Integration tests for component interaction
   - Protocol-specific tests
   - Manual testing for edge cases

## Code Style and Standards

### Documentation

All public APIs must be documented with:

- **Purpose**: What the function/struct does
- **Examples**: Usage examples in doc comments
- **Error Conditions**: When and why errors occur
- **Thread Safety**: Concurrency considerations

Example:
```rust
/// Handles a single stream connection with configurable timeouts
///
/// This function reads data from the stream and echoes it back.
/// It respects the configured timeouts and buffer sizes.
///
/// # Examples
///
/// ```no_run
/// use echosrv::stream::{StreamConfig, StreamEchoServer};
/// use echosrv::tcp::TcpProtocol;
///
/// let config = StreamConfig::default();
/// let server: StreamEchoServer<TcpProtocol> = StreamEchoServer::new(config);
/// ```
///
/// # Errors
///
/// Returns an error if:
/// - Reading from the stream fails
/// - Writing to the stream fails
/// - Timeouts occur
pub async fn handle_connection(/* ... */) -> Result<()> {
    // Implementation
}
```

### Error Handling

#### Library Code
- Use `echosrv::Result` (alias for `Result<T, EchoError>`) for all public APIs
- Define custom error types using `thiserror` derive macro
- Convert underlying errors to appropriate `EchoError` variants
- Don't panic in library code

#### Binary Code
- Use `color_eyre::eyre::Result` for the main binary
- Convert library errors to `eyre::Report` using `.wrap_err()` or automatic conversion
- Provide user-friendly error messages

#### Error Types
```rust
#[derive(Error, Debug)]
pub enum EchoError {
    #[error("TCP error: {0}")]
    Tcp(#[from] std::io::Error),
    
    #[error("UDP error: {0}")]
    Udp(#[from] std::io::Error),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Timeout error: {0}")]
    Timeout(String),
    
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}
```

### Logging

- Use structured logging with `tracing`
- Include relevant context in log messages
- Use appropriate log levels:
  - `error!`: Errors that prevent normal operation
  - `warn!`: Issues that don't prevent operation but should be noted
  - `info!`: Important state changes and operations
  - `debug!`: Detailed information for debugging

### Testing

#### Unit Tests

- Test individual functions in isolation
- Mock external dependencies
- Test both success and error cases
- Use descriptive test names

```rust
#[tokio::test]
async fn test_stream_echo_server_new_creates_valid_instance() {
    let config = StreamConfig::default();
    let server: StreamEchoServer<TcpProtocol> = StreamEchoServer::new(config);
    assert!(server.shutdown_signal().receiver_count() == 0);
}

#[tokio::test]
async fn test_datagram_echo_server_new_creates_valid_instance() {
    let config = DatagramConfig::default();
    let server: DatagramEchoServer<UdpProtocol> = DatagramEchoServer::new(config);
    assert!(server.shutdown_signal().receiver_count() == 0);
}
```

#### Integration Tests

- Test component interaction
- Test real network connections
- Test concurrent scenarios
- Clean up resources properly

```rust
#[tokio::test]
async fn test_multiple_concurrent_tcp_clients() -> Result<()> {
    let (server_handle, addr) = create_controlled_test_server_with_limit(10).await?;
    
    // Test multiple clients concurrently
    let mut handles = Vec::new();
    for i in 0..5 {
        let addr = addr;
        let handle = tokio::spawn(async move {
            let mut client = TcpEchoClient::connect(addr).await?;
            let message = format!("Message from TCP client {}", i);
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), EchoError>(())
        });
        handles.push(handle);
    }
    
    // Wait for all clients to complete
    for handle in handles {
        if let Err(e) = handle.await {
            return Err(EchoError::Config(format!("Task join error: {}", e)));
        }
    }
    
    server_handle.abort();
    Ok(())
}
```

## Protocol Development

### Adding a New Stream Protocol

1. **Implement StreamProtocol trait**:
```rust
pub struct MyStreamProtocol;

impl StreamProtocol for MyStreamProtocol {
    type Error = MyError;
    type Listener = MyListener;
    type Stream = MyStream;
    
    async fn connect(addr: SocketAddr) -> Result<MyStream, MyError> {
        // Implementation
    }
    
    fn bind(config: &StreamConfig) -> impl Future<Output = Result<MyListener, MyError>> + Send {
        // Implementation
    }
    
    // ... other required methods
}
```

2. **Create type aliases**:
```rust
pub type MyEchoServer = StreamEchoServer<MyStreamProtocol>;
pub type MyEchoClient = StreamEchoClient<MyStreamProtocol>;
```

3. **Add tests**:
```rust
#[tokio::test]
async fn test_my_protocol_echo() -> Result<()> {
    let mut client: StreamEchoClient<MyStreamProtocol> = StreamEchoClient::connect(addr).await?;
    let response = client.echo_string("test").await?;
    assert_eq!(response, "test");
    Ok(())
}
```

### Adding a New Datagram Protocol

Similar process but implement `DatagramProtocol` trait instead.

## Performance Considerations

### Memory Management

- Use appropriate buffer sizes (configurable via `Config`)
- Avoid unnecessary allocations in hot paths
- Use `Vec::with_capacity()` when size is known
- Consider using `Bytes` for zero-copy operations in future

### Concurrency

- Use atomic operations for shared state
- Avoid blocking operations in async contexts
- Use appropriate synchronization primitives
- Consider connection pooling for high-load scenarios

### I/O Optimization

- Use async I/O operations consistently
- Implement proper backpressure handling
- Consider using `BufReader`/`BufWriter` for efficiency
- Profile and optimize hot paths

### Protocol-Specific Optimizations

#### Stream Protocols (TCP, Unix streams)
- Connection pooling for high-load scenarios
- Keep-alive connections for repeated requests
- Connection limits to prevent resource exhaustion

#### Datagram Protocols (UDP, Unix datagrams)
- Datagram size optimization
- Connectionless nature allows for stateless scaling
- Consider batching for high-throughput scenarios

## Security Considerations

### Input Validation

- Validate all configuration parameters
- Sanitize log output to prevent injection
- Use appropriate buffer sizes to prevent DoS
- Implement rate limiting if needed

### Resource Management

- Limit maximum connections to prevent DoS (stream protocols)
- Implement timeouts to prevent hanging connections
- Use proper error handling to prevent information leakage
- Consider implementing connection rate limiting

### Protocol-Specific Security

#### Stream Protocols
- Connection limits prevent DoS attacks
- Timeout configuration prevents hanging connections
- Graceful shutdown ensures clean resource cleanup

#### Datagram Protocols
- Stateless nature reduces attack surface
- Datagram size limits prevent amplification attacks
- Timeout configuration prevents resource exhaustion

## Future Enhancements

### Planned Features

1. **Configuration File Support**: Load configuration from files
2. **Metrics Collection**: Prometheus metrics for monitoring
3. **Connection Pooling**: Reuse connections for better performance (stream protocols)
4. **Protocol Extensions**: Support for Unix streams, WebSockets, etc.
5. **TLS Support**: Secure connections with TLS (stream protocols)
6. **DTLS Support**: Secure datagram transport (datagram protocols)

### Architecture Improvements

1. **Plugin System**: Extensible echo behavior
2. **Middleware Support**: Request/response processing pipeline
3. **Health Checks**: Built-in health check endpoints
4. **Graceful Reload**: Configuration reload without restart
5. **Protocol Bridging**: Bridge between stream and datagram protocols

## Contributing

### Pull Request Process

1. **Fork the repository**
2. **Create a feature branch**: `git checkout -b feature/amazing-feature`
3. **Make your changes**: Follow the coding standards
4. **Add tests**: Ensure all new code is tested for both protocols
5. **Update documentation**: Update README.md and DEVELOPMENT.md if needed
6. **Run the test suite**: `cargo test`
7. **Submit a pull request**: Include a clear description of changes

### Commit Message Format

Use conventional commit format:

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

Types:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes
- `refactor`: Code refactoring
- `test`: Adding or updating tests
- `chore`: Maintenance tasks

Scopes:
- `stream`: Stream protocol changes
- `datagram`: Datagram protocol changes
- `tcp`: TCP-specific changes
- `udp`: UDP-specific changes
- `common`: Shared component changes
- `arch`: Architectural changes

### Code Review Guidelines

- Review for correctness and completeness
- Check for security issues
- Ensure proper error handling
- Verify test coverage for both protocols
- Check documentation updates
- Review performance implications

## Troubleshooting

### Common Issues

1. **Connection Refused**: Check if server is running and port is available
2. **Timeout Errors**: Verify timeout configuration and network conditions
3. **Memory Issues**: Check connection limits and buffer sizes
4. **Test Failures**: Ensure no other processes are using test ports
5. **Protocol Mismatch**: Ensure client and server use the same protocol

### Debugging

- Enable debug logging: `RUST_LOG=debug cargo run`
- Use `tracing` spans for request tracing
- Monitor system resources during testing
- Use `cargo test -- --nocapture` to see test output

### Performance Profiling

- Use `cargo bench` for benchmarking (when implemented)
- Profile with `perf` or `flamegraph`
- Monitor memory usage with `valgrind`
- Use `tokio-console` for async runtime debugging

### Protocol-Specific Debugging

#### Stream Protocols
- Monitor connection counts and limits
- Check for connection leaks
- Verify timeout configurations

#### Datagram Protocols
- Monitor datagram sizes and rates
- Check for packet loss
- Verify timeout configurations 