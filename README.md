# Echo Server

A high-performance async echo server library built with Tokio, supporting both TCP and UDP protocols. Perfect for development, testing, and when you need a network service with predictable, verifiable behavior.

## Why Echo Server?

- **Multi-Protocol**: Supports both TCP and UDP protocols
- **Predictable**: Always echoes back exactly what you send - no surprises
- **Simple**: Minimal configuration, just start it and it works
- **Verifiable**: Easy to test - send data, get the same data back
- **Flexible**: Use as a library in your Rust projects or standalone executable
- **Reliable**: Built with proper error handling and connection management
- **High Performance**: Async I/O with Tokio runtime

## Quick Start

### As a Standalone Server

```bash
# Run TCP server on default port 8080
cargo run tcp

# Run UDP server on default port 8080
cargo run udp

# Run TCP server on specific port
cargo run tcp 9000

# Run UDP server on specific port
cargo run udp 9090

# Test TCP with netcat
echo "Hello!" | nc localhost 8080

# Test UDP with netcat
echo "Hello!" | nc -u localhost 8080
```

### As a Library

#### TCP Server

```rust
use echosrv::tcp::{TcpConfig, TcpEchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = TcpConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        max_connections: 100,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = TcpEchoServer::new(config);
    server.run().await?;
    Ok(())
}
```

#### UDP Server

```rust
use echosrv::udp::{UdpConfig, UdpEchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = UdpConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = UdpEchoServer::new(config);
    server.run().await?;
    Ok(())
}
```

## Features

- **Multi-Protocol Support**: Both TCP and UDP echo servers
- **High Performance**: Async I/O with Tokio runtime
- **Connection Limits**: Configurable maximum concurrent connections (TCP)
- **Timeouts**: Configurable read/write timeouts for both protocols
- **Graceful Shutdown**: Responds to SIGINT/SIGTERM
- **Binary Data Support**: Handles any data type, not just text
- **Unicode Support**: Full UTF-8 support
- **Structured Logging**: Built-in observability with tracing
- **Common Interface**: Shared traits for consistent API across protocols

## Use Cases

- **Integration Testing**: When your application requires a network service to be running
- **Network Validation**: Verify network connectivity and port availability
- **Client Testing**: Test network clients that need a server to connect to
- **Development**: Quick setup when you need a service listening on a port
- **Learning**: Understand how TCP/UDP servers work with predictable behavior
- **Protocol Comparison**: Test and compare TCP vs UDP behavior

## Configuration

### TCP Configuration

```rust
let config = TcpConfig {
    bind_addr: "127.0.0.1:8080".parse().unwrap(),
    max_connections: 1000,        // Max concurrent connections
    buffer_size: 1024,            // Read/write buffer size
    read_timeout: Duration::from_secs(30),   // Read timeout
    write_timeout: Duration::from_secs(30),  // Write timeout
};
```

### UDP Configuration

```rust
let config = UdpConfig {
    bind_addr: "127.0.0.1:8080".parse().unwrap(),
    buffer_size: 1024,            // Read/write buffer size
    read_timeout: Duration::from_secs(30),   // Read timeout
    write_timeout: Duration::from_secs(30),  // Write timeout
};
```

## Testing

The library includes test clients for both protocols:

### TCP Client

```rust
use echosrv::tcp::TcpEchoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:8080".parse()?;
    let mut client = TcpEchoClient::connect(addr).await?;
    
    let response = client.echo_string("Hello, TCP Server!").await?;
    println!("Server echoed: {}", response);
    Ok(())
}
```

### UDP Client

```rust
use echosrv::udp::UdpEchoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:8080".parse()?;
    let mut client = UdpEchoClient::connect(addr).await?;
    
    let response = client.echo_string("Hello, UDP Server!").await?;
    println!("Server echoed: {}", response);
    Ok(())
}
```

## Architecture

The library is organized into modules for better maintainability:

- **`common/`**: Shared components, traits, and utilities
- **`tcp/`**: TCP-specific server and client implementations
- **`udp/`**: UDP-specific server and client implementations

### Common Traits

Both TCP and UDP implementations share common traits for consistency:

```rust
use echosrv::common::{EchoServer, EchoClient};

// Both TcpEchoServer and UdpEchoServer implement EchoServer
// Both TcpEchoClient and UdpEchoClient implement EchoClient
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
echosrv = "0.1.0"
```

## License

MIT License - see LICENSE file for details. 