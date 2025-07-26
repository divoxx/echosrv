# Echo Server

A simple TCP echo server for development and testing. When you need a network service running on a port with predictable, verifiable behavior, this server echoes back exactly what you send it.

## Why Echo Server?

- **Predictable**: Always echoes back exactly what you send - no surprises
- **Simple**: Minimal configuration, just start it and it works
- **Verifiable**: Easy to test - send data, get the same data back
- **Flexible**: Use as a library in your Rust projects or standalone executable
- **Reliable**: Built with proper error handling and connection management

## Quick Start

### As a Standalone Server

```bash
# Run on default port 8080
cargo run

# Run on specific port
cargo run 9000

# Test with netcat
echo "Hello!" | nc localhost 8080
```

### As a Library

```rust
use echosrv::tcp::{Config, EchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        bind_addr: "127.0.0.1:8080".parse()?,
        max_connections: 100,
        buffer_size: 1024,
        read_timeout: Some(Duration::from_secs(30)),
        write_timeout: Some(Duration::from_secs(30)),
    };

    let server = EchoServer::new(config);
    server.run().await?;
    Ok(())
}
```

## Features

- **High Performance**: Async I/O with Tokio runtime
- **Connection Limits**: Configurable maximum concurrent connections
- **Timeouts**: Configurable read/write timeouts
- **Graceful Shutdown**: Responds to SIGINT/SIGTERM
- **Binary Data Support**: Handles any data type, not just text
- **Unicode Support**: Full UTF-8 support
- **Structured Logging**: Built-in observability with tracing

## Use Cases

- **Integration Testing**: When your application requires a network service to be running
- **Network Validation**: Verify network connectivity and port availability
- **Client Testing**: Test network clients that need a server to connect to
- **Development**: Quick setup when you need a service listening on a port
- **Learning**: Understand how TCP servers work with predictable behavior

## Configuration

```rust
let config = Config {
    bind_addr: "127.0.0.1:8080".parse().unwrap(),
    max_connections: 1000,        // Max concurrent connections
    buffer_size: 1024,            // Read/write buffer size
    read_timeout: Some(Duration::from_secs(30)),   // Read timeout
    write_timeout: Some(Duration::from_secs(30)),  // Write timeout
};
```

## Testing

The library includes a test client for easy testing:

```rust
use echosrv::tcp::EchoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:8080".parse()?;
    let mut client = EchoClient::connect(addr).await?;
    
    let response = client.echo_string("Hello, Server!").await?;
    println!("Server echoed: {}", response);
    Ok(())
}
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
echosrv = "0.1.0"
```

## License

MIT License - see LICENSE file for details. 