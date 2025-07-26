# Echo Server

A high-performance async echo server library built with Tokio, supporting TCP, UDP, HTTP, and Unix domain socket protocols. Perfect for development, testing, and when you need a network service with predictable, verifiable behavior.

## Why Echo Server?

- **Multi-Protocol**: Supports TCP, UDP, HTTP, and Unix domain sockets (stream and datagram)
- **Predictable**: Always echoes back exactly what you send - no surprises
- **Simple**: Minimal configuration, just start it and it works
- **Verifiable**: Easy to test - send data, get the same data back
- **Flexible**: Use as a library in your Rust projects or standalone executable
- **Reliable**: Built with proper error handling and connection management
- **High Performance**: Async I/O with Tokio runtime
- **Extensible**: Generic architecture supports future protocols

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

# Run Unix domain stream server
cargo run unix-stream /tmp/echo.sock

# Run Unix domain datagram server
cargo run unix-dgram /tmp/echo_dgram.sock

# Run HTTP server on default port 8080
cargo run http

# Run HTTP server on specific port
cargo run http 9000

# Test TCP with netcat
echo "Hello!" | nc localhost 8080

# Test UDP with netcat
echo "Hello!" | nc -u localhost 8080

# Test Unix domain socket with socat
echo "Hello!" | socat - UNIX-CONNECT:/tmp/echo.sock

# Test HTTP with curl
curl -X POST -d "Hello, HTTP!" http://localhost:8080/
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

#### HTTP Server

```rust
use echosrv::http::{HttpConfig, HttpEchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = HttpConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        max_connections: 100,
        buffer_size: 8192,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
        server_name: Some("EchoServer/1.0".to_string()),
        echo_headers: true,
        default_content_type: Some("text/plain".to_string()),
    };

    let server = HttpEchoServer::new(config.into());
    server.run().await?;
    Ok(())
}
```

**Note**: The HTTP echo server only accepts POST requests and echoes back only the request body content (no HTTP headers). Non-POST requests receive a 405 Method Not Allowed response.

#### Unix Domain Stream Server

```rust
use echosrv::unix::{UnixStreamConfig, UnixStreamEchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = UnixStreamConfig {
        socket_path: "/tmp/echo.sock".into(),
        max_connections: 100,
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = UnixStreamEchoServer::new(config);
    server.run().await?;
    Ok(())
}
```

#### Unix Domain Datagram Server

```rust
use echosrv::unix::{UnixDatagramConfig, UnixDatagramEchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = UnixDatagramConfig {
        socket_path: "/tmp/echo_dgram.sock".into(),
        buffer_size: 1024,
        read_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(30),
    };

    let server = UnixDatagramEchoServer::new(config);
    server.run().await?;
    Ok(())
}
```

## Features

- **Multi-Protocol Support**: TCP, UDP, and Unix domain sockets (stream and datagram)
- **High Performance**: Async I/O with Tokio runtime
- **Connection Limits**: Configurable maximum concurrent connections (TCP/Unix stream)
- **Timeouts**: Configurable read/write timeouts for all protocols
- **Graceful Shutdown**: Responds to SIGINT/SIGTERM
- **Binary Data Support**: Handles any data type, not just text
- **Unicode Support**: Full UTF-8 support
- **Structured Logging**: Built-in observability with tracing
- **Common Interface**: Shared traits for consistent API across protocols
- **Generic Architecture**: Extensible for future protocols (WebSockets, TLS, etc.)
- **Unix Domain Sockets**: Efficient inter-process communication on Unix systems

## Use Cases

- **Integration Testing**: When your application requires a network service to be running
- **Network Validation**: Verify network connectivity and port availability
- **Client Testing**: Test network clients that need a server to connect to
- **Development**: Quick setup when you need a service listening on a port
- **Learning**: Understand how TCP/UDP/Unix domain servers work with predictable behavior
- **Protocol Comparison**: Test and compare different transport protocols
- **Inter-Process Communication**: Unix domain sockets for efficient local communication
- **Container Communication**: Unix domain sockets for container-to-container communication

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

### Unix Domain Stream Configuration

```rust
let config = UnixStreamConfig {
    socket_path: "/tmp/echo.sock".into(),    // Unix socket file path
    max_connections: 100,                     // Max concurrent connections
    buffer_size: 1024,                       // Read/write buffer size
    read_timeout: Duration::from_secs(30),   // Read timeout
    write_timeout: Duration::from_secs(30),  // Write timeout
};
```

### Unix Domain Datagram Configuration

```rust
let config = UnixDatagramConfig {
    socket_path: "/tmp/echo_dgram.sock".into(), // Unix socket file path
    buffer_size: 1024,                          // Read/write buffer size
    read_timeout: Duration::from_secs(30),      // Read timeout
    write_timeout: Duration::from_secs(30),     // Write timeout
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

### Unix Domain Stream Client

```rust
use echosrv::unix::UnixStreamEchoClient;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = PathBuf::from("/tmp/echo.sock");
    let mut client = UnixStreamEchoClient::connect(socket_path).await?;
    
    let response = client.echo_string("Hello, Unix Stream Server!").await?;
    println!("Server echoed: {}", response);
    Ok(())
}
```

### HTTP Client

```rust
use echosrv::http::HttpEchoClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:8080".parse()?;
    let mut client = HttpEchoClient::connect(addr).await?;
    
    let response = client.echo_string("Hello, HTTP Server!").await?;
    println!("Server echoed: {}", response);
    Ok(())
}
```

**Note**: The HTTP client sends POST requests and receives only the body content in response.

### Unix Domain Datagram Client

```rust
use echosrv::unix::UnixDatagramEchoClient;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = PathBuf::from("/tmp/echo_dgram.sock");
    let mut client = UnixDatagramEchoClient::connect(socket_path).await?;
    
    let response = client.echo_string("Hello, Unix Datagram Server!").await?;
    println!("Server echoed: {}", response);
    Ok(())
}
```

### Generic Clients

For extensibility, you can also use the generic client implementations:

```rust
use echosrv::stream::StreamEchoClient;
use echosrv::datagram::DatagramEchoClient;
use echosrv::tcp::TcpProtocol;
use echosrv::udp::UdpProtocol;

// Generic stream client with TCP protocol
let mut tcp_client: StreamEchoClient<TcpProtocol> = StreamEchoClient::connect(addr).await?;

// Generic datagram client with UDP protocol
let mut udp_client: DatagramEchoClient<UdpProtocol> = DatagramEchoClient::connect(addr).await?;

// Both work identically to the concrete clients
let response = client.echo_string("Hello!").await?;
```

## Architecture

The library uses a clean, generic architecture for maximum extensibility:

### Module Structure

```
src/
├── common/             # Shared components
│   ├── traits.rs       # Core traits (EchoServerTrait, EchoClient)
│   └── test_utils.rs   # Test utilities
├── stream/             # Generic stream implementation
│   ├── client.rs       # Generic stream client
│   ├── server.rs       # Generic stream server
│   ├── protocol.rs     # StreamProtocol trait
│   └── config.rs       # StreamConfig
├── datagram/           # Generic datagram implementation
│   ├── client.rs       # Generic datagram client
│   ├── server.rs       # Generic datagram server
│   ├── protocol.rs     # DatagramProtocol trait
│   └── config.rs       # DatagramConfig
├── tcp/                # TCP-specific implementation
│   ├── mod.rs          # Type aliases for TCP
│   ├── server.rs       # Type alias: TcpEchoServer = StreamEchoServer<TcpProtocol>
│   ├── config.rs       # TcpConfig
│   └── stream_protocol.rs # TcpProtocol implementation
├── udp/                # UDP-specific implementation
│   ├── mod.rs          # Type aliases for UDP
│   ├── server.rs       # Type alias: UdpEchoServer = DatagramEchoServer<UdpProtocol>
│   ├── config.rs       # UdpConfig
│   └── datagram_protocol.rs # UdpProtocol implementation
├── unix/               # Unix domain socket implementation
│   ├── mod.rs          # Module exports and type aliases
│   ├── config.rs       # UnixStreamConfig, UnixDatagramConfig
│   ├── server.rs       # UnixStreamEchoServer, UnixDatagramEchoServer
│   ├── client.rs       # UnixStreamEchoClient, UnixDatagramEchoClient
│   ├── stream_protocol.rs # UnixStreamProtocol implementation
│   ├── datagram_protocol.rs # UnixDatagramProtocol implementation
│   └── tests.rs        # Unix domain socket tests
├── http/               # HTTP protocol implementation
│   ├── mod.rs          # Module exports and type aliases
│   ├── config.rs       # HttpConfig
│   ├── protocol.rs     # HttpProtocol implementation
│   ├── client.rs       # HttpEchoClient type alias
│   └── tests.rs        # HTTP protocol unit tests
├── lib.rs              # Main library exports
└── main.rs             # Binary entry point
```

### Design Philosophy

- **Generic Architecture**: Stream and datagram clients/servers are generic over protocol implementations
- **Type Aliases**: Concrete clients (`TcpEchoClient`, `UdpEchoClient`) are type aliases to generic implementations
- **Protocol Traits**: `StreamProtocol` and `DatagramProtocol` traits define the interface for protocol implementations
- **Extensibility**: Easy to add new protocols (Unix streams, WebSockets, etc.) by implementing the protocol traits

### Common Traits

Both TCP and UDP implementations share common traits for consistency:

```rust
use echosrv::common::{EchoServerTrait, EchoClient};

// Both TcpEchoServer and UdpEchoServer implement EchoServerTrait
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