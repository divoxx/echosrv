# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Rate-limiting primitives** in the new `rate_limit` module (re-exported at
  the crate root): `Gcra`, a lock-free GCRA policer (`check()` /
  `check_at()` admit an event or return `RateLimited { retry_after }`), and
  `TokenBucket`, a cancel-safe async shaper (`acquire()`), both configured by
  `RateLimitConfig { rate_per_sec, burst }`. `RateLimitError` is the
  matching error enum.
- **Server rate limits.** `rate_limit: Option<RateLimitConfig>` on every
  server config (`StreamConfig`, `DatagramConfig`, `TcpConfig`, `UdpConfig`,
  `HttpConfig`, `UnixStreamConfig`, `UnixDatagramConfig`) limits the request
  rate of the whole server, and `accept_rate_limit` on the stream configs
  limits new connections. Set them with `with_rate_limit` /
  `with_accept_rate_limit`. Over-limit traffic is rejected, not delayed:
  - TCP resets the connection (`SO_LINGER` 0).
  - HTTP answers `429 Too Many Requests` with `Retry-After` (whole seconds,
    rounded up, at least 1) and `Connection: close`. A connection over the
    accept limit has its request head read first, so the `429` is not lost
    to a reset.
  - Unix stream closes the connection.
  - UDP and Unix datagram drop the datagram.
- **`ServerStats`** (`rejected_requests`, `rejected_connections`,
  `rejected_over_capacity`, `dropped_rate_limited`), available from `stats()` on every server and bound
  server.
- **`StreamProtocol` hooks** with default implementations: `reject(stream,
  RejectReason, retry_after)`, `FRAMED_REQUESTS` and `begin_request`.
- **CLI rate-limit and logging flags:** `--rate`, `--burst` (default: the
  rate), `--accept-rate`, `--accept-burst` (default: the accept rate),
  `--max-connections` (default 1000 for tcp/http, 100 for unix-stream) and
  `--log-level` (default `info`; `RUST_LOG` still overrides it). The help
  states every default.
- **`defaults` module** with the default protocol, host, port and Unix socket
  paths used by the binary and the Unix configs.
- **Client error classification:** `EchoError::io_error_kind()` (the
  `std::io::ErrorKind` of `Tcp`/`Udp`/`Unix` errors), `is_rate_limited()`
  (HTTP `429`) and `retry_after()`.
- **`EchoError::HttpStatus { status, reason, retry_after, body }`**, returned
  by `HttpEchoClient` for a non-2xx response; `retry_after` is the parsed
  `Retry-After` header (delay-seconds form).
- **`echosrv-client`**, a load-testing binary for the echo servers. It runs
  `-n` requests or continuously (`-d`, Ctrl-C) over `-c` workers, with
  persistent or per-request connections, checks every echo byte for byte, and
  reports live intervals, outage start/end events and a summary (latency
  percentiles, errors by kind, outages) as text or JSON lines (`--json`). It
  can shape traffic (`--rate`/`--burst`), honor HTTP `Retry-After`, and caps
  new connections at 100/s by default (`--conn-rate`) with exponential
  backoff after errors, so by default it does not exhaust the machine's
  ephemeral ports. `SIGTERM`, Ctrl-C and the end of `--duration` stop it
  gracefully: requests in flight finish and are counted. Its exit status
  reflects the run (`--max-error-rate`). See the README.

### Changed

- **CLI:** the argument parser is now clap. Arguments and behavior are the
  same (positional `[PROTOCOL] [PORT|SOCKET_PATH]`, `--host`, exit status 1 on
  usage errors), but error messages use clap's wording, for example
  "unexpected argument '--bogus'" instead of "unknown option '--bogus'".
  `--accept-rate` and `--max-connections` are rejected for `udp` and
  `unix-dgram`. Logs are colored only when stderr is a terminal and
  `NO_COLOR` is unset.
- **Breaking:** the server config structs gained `rate_limit` (and, for
  stream configs, `accept_rate_limit`). Struct literals without
  `..Default::default()` must add them.
- A zero `rate_per_sec` or `burst` in a server's rate limit is a
  configuration error at `bind()`.
- HTTP: an invalid request now counts as one request for the rate limit; its
  error response is sent after the request has been admitted.
- **Breaking:** clients no longer return a partial echo as success.
  - The stream clients (`TcpEchoClient`, `UnixStreamEchoClient`) fail with an
    `UnexpectedEof` I/O error when the server closes the connection before
    the whole echo arrived (they used to return the bytes received so far).
  - `HttpEchoClient` reports a non-2xx response as `EchoError::HttpStatus`
    instead of `EchoError::Http(String)` (the message is unchanged), and a
    connection closed before the response head or body is complete as an
    `UnexpectedEof` `Tcp` error instead of `EchoError::Http`.
  - The datagram clients (`UdpEchoClient`, `UnixDatagramEchoClient`) fail
    with `EchoError::Config` when a reply is larger than
    `DatagramClientConfig::buffer_size`, instead of truncating it.
- `HttpEchoClient` reads responses in chunks of `ClientConfig::buffer_size`
  (it used a fixed 8 KiB buffer).
- **Breaking:** `Address` has two new variants, `UnixAbstract(Vec<u8>)` (a
  Linux abstract-namespace socket, written `unix:@name`) and `UnixUnnamed`.
  Exhaustive matches on `Address` must handle them. `"unix:@name"` now parses
  as `UnixAbstract`; write `unix:./@name` for a file whose name starts with
  `@`. `Address::is_unix()` is true for all three Unix variants, and
  `Address::as_unix_abstract()` returns the abstract name. Unix stream clients
  can connect to `UnixAbstract` addresses on Linux. The minimum Tokio version
  is now 1.41.

### Fixed

- Inheriting an abstract-namespace Unix socket (systemd
  `ListenStream=@name`) or an unnamed one no longer fails at `bind()` with
  `AddrNotAvailable`: `local_addr()` reports it as `Address::UnixAbstract` or
  `Address::UnixUnnamed`. Inherited sockets are never unlinked, so an
  abstract socket is never mistaken for a socket file.
- The benchmarks reuse their connections instead of connecting on every
  iteration, so `cargo bench` opens a few dozen connections rather than tens
  of thousands (which could exhaust the ephemeral port range with TIME_WAIT
  sockets). `tcp_echo` and `tcp_raw` now measure the echo round trip only,
  without connection setup.
- HTTP servers report bind errors with the same `EchoError` variant as TCP
  servers. An unusable inherited socket is `EchoError::FdInheritance` and an
  invalid bind target is `EchoError::Config`; both used to be wrapped as
  `EchoError::Tcp` ("TCP error: FD inheritance error: ..."). The new
  `HttpProtocolError::Bind` variant carries the original `EchoError` and
  converts back into it unchanged.
- The datagram servers (UDP, Unix datagram) no longer spin in a busy loop
  when `recv_from` keeps failing. They wait 100 ms after each failed receive
  (shutdown still interrupts the wait) and log the first failure and then
  every 100th at `error`, the others at `debug`.
- **Stale Unix socket detection no longer removes a live server's socket.**
  The probe connect is non-blocking, so it no longer stalls a runtime thread
  on Linux when the listener's backlog is full (that answer, `EAGAIN`, now
  counts as live). Unix stream servers hold an advisory lock on
  `<path>.lock` while bound, because macOS and the BSDs report a full backlog
  as `ECONNREFUSED`, the same as a dead socket. A server finding the lock
  held reports `AddrInUse` without touching the socket file.
- Stream connections over `max_connections` are rejected through
  `StreamProtocol::reject` with the new `RejectReason::TooManyConnections`
  instead of being dropped silently: HTTP answers `503 Service Unavailable`
  with `Connection: close`, TCP resets the connection and Unix stream closes
  it. They are counted in `ServerStats::rejected_over_capacity`.
- Connection rejections (over `accept_rate_limit` or `max_connections`) no
  longer take `max_connections` slots. They run in a separate pool of at most
  32, and connections rejected while it is full are closed at once, so a
  flood of rejected connections cannot lock out admitted clients.
- **Breaking (minor):** `RejectReason` has the new variant
  `TooManyConnections`. Exhaustive matches on it must handle it.
- `echosrv-client` fails a run with no attempts (exit status 1, verdict
  `[fail] no requests were attempted`), for example one stopped by Ctrl-C
  before any request completed; it used to pass with exit status 0. The
  config header prints whole hours as `1h` instead of `60m`.

## [0.4.0] - Unreleased

This release makes the servers behave as documented. HTTP now speaks real
HTTP/1.1, socket inheritance works for every protocol, shutdown is reliable,
and the tests run against real servers. It contains many breaking changes; see
below.

### Breaking

- **Config structs gained fields.** `StreamConfig`, `DatagramConfig`,
  `TcpConfig`, `UdpConfig` and `HttpConfig` now have `bind_strategy:
  Option<BindStrategy>` and `service_name: String`. Struct literals must add
  `..Default::default()`.
- **Unix configs:** `UnixStreamConfig::socket_path` and
  `UnixDatagramConfig::socket_path` are replaced by `bind_strategy:
  BindStrategy` and `service_name`. Use `with_socket_path(path)` or
  `with_fd_inheritance(name, fallback_path)`.
- **`EchoError` has new variants** `FdInheritance(String)` and `Http(String)`.
  Exhaustive matches must handle them.
- **`Address`:** the panicking `From<&str>` is replaced by `TryFrom<&str>`
  (and `FromStr`). An empty `"unix:"` path is rejected.
- **HTTP:**
  - `HttpEchoServer` is now a struct (it was an alias of
    `StreamEchoServer<HttpProtocol>`). It is built from `HttpConfig` and has
    its own `bind()`.
  - `HttpEchoClient` is now a real HTTP client struct (it was
    `Client<HttpProtocol>`). It returns the response body and fails on
    non-2xx responses.
  - `HttpConfig::echo_headers` was removed (it was never implemented).
    `HttpConfig::max_body_size` was added (default 1 MiB).
  - `HttpConfig::default()` now binds `127.0.0.1:0` instead of
    `127.0.0.1:8080`.
  - Responses are now full HTTP/1.1 responses instead of the raw body (see
    Fixed).
- **`UnixStreamEchoClient`** is now an alias of `Client<UnixStreamProtocol>`.
  It shares `ClientConfig` timeouts and the response size limit with the
  other stream clients.
- **Unix protocol socket types:** `UnixStreamProtocol::Listener` is
  `ManagedUnixListener` and `UnixDatagramProtocol::Socket` is
  `ManagedUnixDatagram`. These wrappers own (and remove) the socket file.
- **`StreamProtocol` trait:**
  - `Listener` must implement `network::LocalAddress`.
  - New methods: `bind_with_inheritance` (default calls `bind`) and
    `connect_address` (default handles network addresses).
- **`DatagramProtocol` trait:**
  - New associated type `PeerAddr`. `recv_from` and `send_to` use it instead
    of `SocketAddr`.
  - `Socket` must be `Sync + LocalAddress`.
  - New method `bind_with_inheritance`.
- **`stream::Client<P>`** now requires `P::Stream: AsyncRead + AsyncWrite +
  Unpin`, so it can read while it writes.
- **Datagram buffers:** the default `buffer_size` for UDP/Unix datagram
  servers and clients is now 64 KiB (`DEFAULT_DATAGRAM_BUFFER_SIZE`) instead
  of 1024 bytes. The client buffer is configurable through
  `DatagramClientConfig`.
- **Signals:** the library no longer installs a Ctrl-C handler. `run()` only
  returns after `shutdown_signal().send(())`. The `echosrv` binary handles
  `SIGINT` and `SIGTERM` itself.
- **Platforms:** the crate now fails to compile on non-Unix targets
  (`compile_error!`).
- **CLI:** an invalid port, an unknown protocol or extra arguments are now
  errors. Previously a bad port silently fell back to 8080.

### Added

- **Socket inheritance and systemd socket activation** for TCP, UDP, HTTP,
  Unix stream and Unix datagram (`network::fd_inheritance`):
  - `BindStrategy::{Bind(BindTarget), Inherit(InheritedFd), InheritOrBind {
    fd: Option<InheritedFd>, fallback_target: Option<BindTarget> }}`.
  - `InheritedFd`, a take-once handle around an `OwnedFd`.
  - `FdInheritanceConfig`, a shared pool of named descriptors.
    `from_systemd_env()` parses `LISTEN_PID`/`LISTEN_FDS`/`LISTEN_FDNAMES`
    once per process. `from_fds()` builds a pool for custom process managers.
    `take_named_or_sole()` picks a descriptor.
  - `with_fd_inheritance(service_name)` on every config.
  - Inherited descriptors are validated: socket type, address family, and
    listening state for stream sockets.
- **`bind()`** on every server returns a `'static` bound server (for example
  `BoundStreamServer`, `BoundDatagramServer` or `BoundHttpServer`) with
  `local_addr()` and `serve()`. Use it to bind port 0 and learn the real
  address.
- **HTTP/1.1 framing:** `Content-Length` bodies, `Expect: 100-continue`,
  `Connection: close`, and status codes 400, 405 (with `Allow: POST`), 413,
  431 and 501. `MAX_HEADER_BYTES` (8 KiB), `MAX_HEADERS` (32) and
  `DEFAULT_MAX_BODY_SIZE` (1 MiB) are public.
- **CLI:**
  - `--host <ADDR>` (IPv4 or IPv6), `-h/--help` and `-V/--version`.
  - `unix-datagram` as an alias of `unix-dgram`.
  - `RUST_LOG` filtering (default `echosrv=info`).
  - Graceful shutdown on `SIGTERM` as well as `SIGINT`.
  - systemd socket activation (the socket named after the protocol, or the
    only socket passed).
- **Stale Unix socket recovery:** a leftover socket file that nobody listens
  on is replaced at bind time, and missing parent directories are created.
- `EchoError::into_io_error()`.
- `ClientConfigBuilder` and `DatagramClientConfig`.
- Tests: per-protocol integration suites (`tests/{tcp,udp,unix,http,fd_inheritance,cli}.rs`),
  shared helpers in `tests/common/`, and README examples compiled as
  doctests.

### Fixed

- **HTTP:**
  - The server now sends a real `200 OK` response with headers. Before, it
    wrote the raw body with no status line.
  - Requests whose head arrives in several segments are now handled.
  - Request bodies are framed correctly.
  - An empty `POST` gets a response.
  - Error responses are no longer lost to a TCP reset.
- **Unix sockets:**
  - Servers no longer unconditionally delete whatever is at the socket
    path. Before, they could remove a running server's socket or a regular
    file. Only stale socket files are replaced now.
  - The socket file is removed on shutdown only if this server created it.
    Inherited sockets are never removed.
  - The datagram client works on macOS (no `EISCONN`).
  - The datagram client removes its temporary socket on drop.
  - The Unix stream server now honors `max_connections`. Both Unix servers
    now share the generic accept loop, timeouts and shutdown handling.
- **Shutdown:**
  - A shutdown sent before `run()` is no longer lost.
  - In-flight connections are cancelled and awaited instead of outliving the
    server.
- **Connection limit:** fixed a race (load-then-increment) that allowed more
  than `max_connections`. The connection slot is now released even if a
  connection task panics.
- **Accept errors:** a failed `accept()` (for example `EMFILE`) now backs off
  instead of spinning.
- **Large payloads:** stream and HTTP clients no longer deadlock on payloads
  larger than the socket buffers.
- **UDP:**
  - Datagrams larger than 1024 bytes are no longer truncated by default.
  - IPv6 clients are supported.
- `Address::from("garbage")` no longer panics (see `TryFrom`).
- Benchmarks compile and run against a real server address.

### Removed

- The `security` module (`RateLimiter`, `ConnectionTracker`, `SizeValidator`,
  `ResourceLimits`) and the `performance` module (`BufferPool`,
  `PooledBuffer`). Neither was used by any server.
- `network::Config` (the unused builder-style config) and
  `common::test_utils` (`create_controlled_test_server_with_limit`, which was
  racy; a test-only replacement is in `tests/common/`).
- The `bytes` dependency.
- The `tests/integration_tests.rs` and `tests/comprehensive_integration.rs`
  suites, which were replaced by the per-protocol suites.

## [0.3.0] - 2024-12-19

> **Note (corrected in 0.4.0):** Parts of this entry were inaccurate:
> - 0.3.0 *did* contain breaking changes, for example `StreamEchoClient` was
>   renamed to `Client`.
> - The `security` and `performance` modules were never used by any server.
>   They were removed in 0.4.0.
> - The "~60% reduction in memory allocations" was never measured.
> - UDP has no connection limits, and the TCP limit had a race.
> - There was no cross-platform CI.
>
> The original text is kept below for reference.

### Added

#### Core Features
- **Unified Address System**: New `Address` enum supporting both network (`SocketAddr`) and Unix domain socket (`PathBuf`) addresses
- **Enhanced Configuration System**: Fluent builder pattern with `Config` type for protocol-agnostic configuration
- **Security & Resource Management**: Comprehensive rate limiting, connection tracking, and size validation
- **Performance Optimizations**: Buffer pooling system with reusable buffers and global pool management
- **Improved Client Library**: Configurable timeouts, size limits, and idle connection detection

#### Security Features
- Rate limiting with permit-based system (`RateLimiter`)
- Connection tracking with automatic cleanup (`ConnectionTracker`)
- Request size validation (`SizeValidator`)
- Resource limits with configurable thresholds (`ResourceLimits`)

#### Performance Features  
- Buffer pooling with RAII management (`BufferPool`, `PooledBuffer`)
- Global buffer pool for application-wide buffer reuse
- Zero-copy operations with `bytes::Bytes` integration
- Reduced memory allocations in hot paths

#### Enhanced Testing
- **Property-based testing** with Proptest for data preservation validation
- **Performance benchmarks** with Criterion for throughput measurement
- **Comprehensive integration tests** covering all protocols and features
- **Concurrent testing** scenarios for stress testing

#### New Modules
- `src/network/` - Unified addressing and configuration system
- `src/security/` - Resource limits and protection mechanisms  
- `src/performance/` - Buffer pooling and optimization utilities

### Changed

#### API Improvements
- **Ergonomic Address API**: `TcpEchoClient::connect()` now accepts `SocketAddr`, `&str`, or `Address` via `Into<Address>`
- **FromStr Support**: `Address` type now implements `FromStr` for parsing from strings
- **Improved Error Types**: Better error context and structured error handling throughout

#### Client Enhancements
- Configurable timeouts for connect, read, and write operations
- Size limits to prevent memory exhaustion
- Idle connection detection and management
- Enhanced timeout handling replacing fixed 200ms timeouts

#### Testing Infrastructure
- Property-based tests ensure data preservation across all scenarios
- Concurrent client testing validates thread safety
- Performance regression testing with benchmarks
- Cross-platform compatibility testing

### Fixed

#### Connection Management
- Proper port binding for test servers (fixed port 0 connection issues)
- Improved resource cleanup in all protocols
- Better timeout handling and error propagation

#### Protocol Improvements
- Unix domain socket cleanup and resource management
- HTTP protocol buffer management and request parsing
- TCP/UDP connection limit enforcement
- Error handling consistency across all protocols

### Technical Details

#### Architecture
- **Generic Protocol System**: Maintained zero-cost abstractions with type aliases
- **Semantic Module Organization**: Domain-based modules (network, security, performance) instead of generic utils
- **RAII Resource Management**: Automatic cleanup for connections, buffers, and sockets
- **Backward Compatibility**: All existing APIs remain unchanged

#### Dependencies
- Updated to `thiserror = "2"` for improved error handling
- Added `bytes = "1.4"` for efficient buffer management
- Added `proptest = "1.0"` for property-based testing
- Added `criterion = "0.5"` for performance benchmarking

#### Performance Metrics
- ~60% reduction in memory allocations through buffer pooling
- Improved connection management with atomic counters
- Optimized network operations with better buffer handling
- Enhanced concurrent performance with proper resource limits

### Migration Guide

#### For Existing Users
- **No Breaking Changes**: All existing code continues to work without modification
- **Optional Upgrades**: New features are opt-in and additive
- **Enhanced APIs**: Existing APIs now support more input types (e.g., string addresses)

#### New Feature Adoption
```rust
// Old way (still works)
let mut client = TcpEchoClient::connect(&addr).await?;

// New ergonomic way
let mut client = TcpEchoClient::connect("127.0.0.1:8080").await?;
let mut client = TcpEchoClient::connect(socket_addr).await?;

// New unified addressing
let tcp_addr: Address = "127.0.0.1:8080".parse()?;
let unix_addr: Address = "unix:/tmp/echo.sock".into();
```

## [0.2.0] - 2024-12-19

### Added
- HTTP protocol support with POST-only echo functionality
- Comprehensive error handling with `thiserror`
- Enhanced logging with `tracing`
- Binary entry point for standalone server usage

### Changed
- Updated to `thiserror = "2"` for better error handling
- Improved async trait implementations

## [0.1.0] - Initial Release

### Added
- Basic TCP echo server and client
- UDP echo server and client  
- Unix domain socket support (stream and datagram)
- Generic protocol architecture with type aliases
- Async/await support with Tokio
- Basic configuration system
- Integration tests for all protocols