# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

EchoSrv is a high-performance async echo server library built with Tokio that supports multiple protocols (TCP, UDP, HTTP, Unix domain sockets). It uses a generic architecture where concrete protocol implementations are type aliases over generic components.

## Build and Development Commands

```bash
# Build the project
cargo build

# Run tests (unit tests in src/, integration tests in tests/)
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test module
cargo test --test integration_tests test_tcp
cargo test --test integration_tests test_udp  
cargo test --test integration_tests test_http

# Run HTTP unit tests specifically
cargo test http::tests

# Code quality
cargo clippy
cargo fmt

# Run server examples
cargo run tcp          # TCP on port 8080
cargo run udp 9090     # UDP on specific port
cargo run http         # HTTP on port 8080
cargo run unix-stream /tmp/echo.sock
```

## Core Architecture

### Generic Protocol System
- **Generic servers**: `StreamEchoServer<P>` and `DatagramEchoServer<P>` over protocol traits
- **Protocol traits**: `StreamProtocol` and `DatagramProtocol` define interfaces
- **Type aliases**: Concrete types like `TcpEchoServer = StreamEchoServer<TcpProtocol>`
- **Shared traits**: `EchoServerTrait` and `EchoClient` for consistent API

### Module Organization
```
src/
├── common/         # Shared traits and test utilities
├── stream/         # Generic stream implementation (servers/clients)
├── datagram/       # Generic datagram implementation (servers/clients)  
├── tcp/            # TCP protocol + type aliases
├── udp/            # UDP protocol + type aliases
├── unix/           # Unix domain sockets (stream & datagram)
├── http/           # HTTP protocol (POST-only echo)
└── lib.rs          # Error types + re-exports
```

### Key Design Patterns
- **Zero-cost abstractions**: Type aliases over generics
- **Protocol extensibility**: Add new protocols by implementing traits
- **Semantic modules**: Domain-based organization (not utils/helpers)
- **Connection management**: Atomic counting with configurable limits (stream protocols)

## Error Handling
- Library uses `echosrv::Result<T>` and structured `EchoError` types
- Binary uses `color-eyre` for user-friendly error reporting
- All errors provide context and are properly propagated

## Testing Strategy
- **Unit tests**: In each module's `tests.rs` or inline
- **Integration tests**: In `tests/integration_tests.rs`
- **Protocol coverage**: Tests for all protocols (TCP, UDP, HTTP, Unix)
- **Concurrency testing**: Multiple concurrent clients
- **Error scenarios**: Timeout, connection limits, malformed requests

## HTTP Protocol Notes
- **POST-only**: Only accepts POST requests, returns 405 for others
- **Body echo**: Echoes only request body content (no HTTP headers)
- **Buffer management**: Handles incomplete requests and large payloads
- **Error responses**: Proper HTTP status codes for non-POST methods

## Recent Improvements (v0.2.0+)
- **Unified Address System**: `src/network/address.rs` - Support for both network and Unix addresses
- **Security Features**: `src/security/limits.rs` - Rate limiting, connection tracking, size validation
- **Performance Optimizations**: `src/performance/buffer_pool.rs` - Buffer pooling, reduced allocations
- **Enhanced Client**: `src/stream/improved_client.rs` - Better timeout handling, configuration
- **Property-Based Tests**: `tests/property_tests.rs` - Comprehensive validation with random data
- **Performance Benchmarks**: `benches/echo_performance.rs` - Throughput and concurrency testing

## Build and Test Commands
```bash
# Standard development
cargo build
cargo test
cargo clippy
cargo fmt

# Performance testing
cargo bench                    # Run benchmarks
cargo test --test property_tests  # Property-based tests

# Specific test categories
cargo test --test comprehensive_integration  # Integration tests
cargo test test_security_features           # Security validation
cargo test test_performance_optimizations   # Performance features
```

## Important Files
- `IMPLEMENTATION_SUMMARY.md`: Detailed overview of recent improvements and architecture
- `DEVELOPMENT.md`: Comprehensive development guide and architecture details
- `tests/integration_tests.rs`: Cross-protocol integration tests
- `tests/property_tests.rs`: Property-based testing with random data validation
- `src/lib.rs`: Main error types and module re-exports
- `src/common/traits.rs`: Core traits shared across protocols
- `src/network/`: Unified addressing and configuration system
- `src/security/`: Resource limits and protection mechanisms
- `src/performance/`: Buffer pooling and optimization utilities