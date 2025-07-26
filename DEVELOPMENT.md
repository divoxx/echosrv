# Development Guide

This document outlines the development practices, architecture decisions, and guidelines for contributing to the Echo Server project.

## Architecture Overview

### System Design

The Echo Server is built around a simple but robust architecture:

1. **Server Management**: The `EchoServer` manages the TCP listener and connection lifecycle
2. **Configuration**: The `Config` struct holds all server parameters
3. **Client Testing**: The `EchoClient` provides functionality for testing the server
4. **Connection Handling**: Individual async tasks handle each client connection

### Module Structure

```
src/
├── lib.rs          # Library entry point, exposes tcp module
├── main.rs         # Binary entry point for standalone server
└── tcp.rs          # Core TCP server implementation
```

### Key Design Decisions

#### 1. Async Architecture
- **Implementation**: Uses Tokio runtime for async/await support
- **Benefits**: Efficient resource utilization, high concurrency

#### 2. Error Handling
- **Implementation**: Uses `color-eyre` for error handling with context
- **Benefits**: Better debugging experience, clear error messages

#### 3. Logging
- **Implementation**: Uses `tracing` for structured logging
- **Benefits**: Configurable log levels, structured data, performance

#### 4. Connection Management
- **Implementation**: Atomic connection counting with configurable limits
- **Benefits**: Predictable resource usage, graceful degradation

#### 5. Timeout Configuration
- **Implementation**: Configurable read/write timeouts per connection
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
/// Handles a single TCP connection with configurable timeouts
///
/// This function reads data from the socket and echoes it back.
/// It respects the configured timeouts and buffer sizes.
///
/// # Examples
///
/// ```no_run
/// use echosrv::tcp::{Config, EchoServer};
///
/// let config = Config::default();
/// let server = EchoServer::new(config);
/// ```
///
/// # Errors
///
/// Returns an error if:
/// - Reading from the socket fails
/// - Writing to the socket fails
/// - Timeouts occur
pub async fn handle_connection(/* ... */) -> Result<()> {
    // Implementation
}
```

### Error Handling

- Use `color-eyre::eyre::Result` for all public APIs
- Provide context for errors using `.with_context()`
- Log errors at appropriate levels (error, warn, info)
- Don't panic in library code

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
async fn test_echo_server_new_creates_valid_instance() {
    let config = Config::default();
    let server = EchoServer::new(config);
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
async fn test_multiple_concurrent_clients() -> Result<()> {
    let (server_handle, addr) = create_test_server().await?;
    
    // Test multiple clients concurrently
    let mut handles = Vec::new();
    for i in 0..5 {
        let addr = addr;
        let handle = tokio::spawn(async move {
            let mut client = EchoClient::connect(addr).await?;
            let message = format!("Message from client {}", i);
            let response = client.echo_string(&message).await?;
            assert_eq!(response, message);
            Ok::<(), color_eyre::eyre::Error>(())
        });
        handles.push(handle);
    }
    
    // Wait for all clients to complete
    for handle in handles {
        handle.await??;
    }
    
    server_handle.abort();
    Ok(())
}
```

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

## Security Considerations

### Input Validation

- Validate all configuration parameters
- Sanitize log output to prevent injection
- Use appropriate buffer sizes to prevent DoS
- Implement rate limiting if needed

### Resource Management

- Limit maximum connections to prevent DoS
- Implement timeouts to prevent hanging connections
- Use proper error handling to prevent information leakage
- Consider implementing connection rate limiting

## Future Enhancements

### Planned Features

1. **Configuration File Support**: Load configuration from files
2. **Metrics Collection**: Prometheus metrics for monitoring
3. **Connection Pooling**: Reuse connections for better performance
4. **Protocol Extensions**: Support for custom protocols
5. **TLS Support**: Secure connections with TLS

### Architecture Improvements

1. **Plugin System**: Extensible echo behavior
2. **Middleware Support**: Request/response processing pipeline
3. **Health Checks**: Built-in health check endpoints
4. **Graceful Reload**: Configuration reload without restart

## Contributing

### Pull Request Process

1. **Fork the repository**
2. **Create a feature branch**: `git checkout -b feature/amazing-feature`
3. **Make your changes**: Follow the coding standards
4. **Add tests**: Ensure all new code is tested
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

### Code Review Guidelines

- Review for correctness and completeness
- Check for security issues
- Ensure proper error handling
- Verify test coverage
- Check documentation updates
- Review performance implications

## Troubleshooting

### Common Issues

1. **Connection Refused**: Check if server is running and port is available
2. **Timeout Errors**: Verify timeout configuration and network conditions
3. **Memory Issues**: Check connection limits and buffer sizes
4. **Test Failures**: Ensure no other processes are using test ports

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