# echosrv

Async echo servers and clients for TCP, UDP, HTTP and Unix domain sockets,
built on Tokio. It ships as a command-line tool and as a library. Use it as a
predictable peer in tests, for debugging network setups, or as a socket
activation playground.

Every server sends back exactly the bytes it receives. The HTTP server echoes
the body of each `POST` request.

**Platform:** Unix-like systems only (Linux, macOS, BSD). The crate does not
compile on Windows.

| Protocol           | CLI name                        | Server                   | Client                   |
|--------------------|---------------------------------|--------------------------|--------------------------|
| TCP                | `tcp`                           | `TcpEchoServer`          | `TcpEchoClient`          |
| UDP                | `udp`                           | `UdpEchoServer`          | `UdpEchoClient`          |
| HTTP/1.1           | `http`                          | `HttpEchoServer`         | `HttpEchoClient`         |
| Unix stream        | `unix-stream`                   | `UnixStreamEchoServer`   | `UnixStreamEchoClient`   |
| Unix datagram      | `unix-dgram` / `unix-datagram`  | `UnixDatagramEchoServer` | `UnixDatagramEchoClient` |

## Installation

Command-line tool:

```bash
cargo install echosrv
```

Library:

```toml
[dependencies]
echosrv = "0.4"
tokio = { version = "1", features = ["full"] }
```

Minimum supported Rust version: 1.85 (edition 2024).

## Command line

```text
Async echo server for TCP, UDP, HTTP and Unix domain sockets

Usage: echosrv [OPTIONS] [PROTOCOL] [PORT|SOCKET_PATH]

Arguments:
  [PROTOCOL]          Protocol to serve [default: tcp] [possible values: tcp, udp, http, unix-stream, unix-dgram]
  [PORT|SOCKET_PATH]  Port for tcp/udp/http, or socket path for unix-stream/unix-dgram [default: 8080 for tcp/udp/http, /tmp/echosrv_stream.sock for unix-stream, /tmp/echosrv_datagram.sock for unix-dgram]

Options:
      --host <ADDR>            IP address (IPv4 or IPv6) to bind for tcp/udp/http [default: 127.0.0.1]
      --rate <PER_SEC>         Request rate limit in requests/s (datagrams/s for udp/unix-dgram); excess is rejected [default: unlimited]
      --burst <N>              Request burst size [default: same as --rate]
      --accept-rate <PER_SEC>  New-connection rate limit in connections/s (tcp, http, unix-stream); excess is rejected [default: unlimited]
      --accept-burst <N>       New-connection burst size [default: same as --accept-rate]
      --max-connections <N>    Maximum concurrent connections (tcp, http, unix-stream) [default: 1000 for tcp/http, 100 for unix-stream]
      --log-level <LEVEL>      Log level (RUST_LOG overrides it) [default: info] [possible values: off, error, warn, info, debug, trace]
  -h, --help                   Print help (see more with '--help')
  -V, --version                Print version
```

Examples:

```bash
# TCP on 127.0.0.1:8080
echosrv
echo hello | nc 127.0.0.1 8080

# UDP on all interfaces, port 9090
echosrv --host 0.0.0.0 udp 9090
echo hello | nc -u -w1 127.0.0.1 9090

# IPv6 loopback
echosrv --host ::1 tcp 8080

# HTTP: echoes the request body
echosrv http 8080
curl --data-binary 'hello' http://127.0.0.1:8080/   # -> hello
curl -i http://127.0.0.1:8080/                      # -> 405 Method Not Allowed

# Rate limits: 100 requests/s (bursts of 10) and 20 new connections/s
echosrv http 8080 --rate 100 --burst 10 --accept-rate 20
curl -i --data-binary 'hi' http://127.0.0.1:8080/   # over the limit -> 429 + Retry-After

# Unix domain sockets
echosrv unix-stream /tmp/echo.sock
echo hello | nc -U /tmp/echo.sock
echosrv unix-dgram /tmp/echo_dgram.sock
```

From a checkout, run it with `cargo run -- <args>`, for example
`cargo run -- http 8080`.

`echosrv --help` describes each option in more detail, with its default on a
line of its own.

**Rate limits.** `--rate`/`--burst` limit requests (datagrams for `udp` and
`unix-dgram`) and `--accept-rate`/`--accept-burst` limit new connections, each
for the whole server. Over-limit traffic is rejected, never delayed: see
[Rate limiting](#rate-limiting). `--accept-rate` and `--max-connections` are
errors for the datagram protocols, like `--host` for the Unix ones.

**Logging.** Logs go to stderr through `tracing`. `--log-level` sets the level
of echosrv's own logs (default `info`, so `echosrv=info`). `RUST_LOG`, when
set, overrides it with a full filter: use `RUST_LOG=echosrv=debug` to see
connections and rejections, or `RUST_LOG=echosrv=trace` to see payloads. Logs
are colored only when stderr is a terminal and `NO_COLOR` is not set.

**Signals.** `SIGINT` (Ctrl-C) and `SIGTERM` trigger a graceful shutdown. The
server stops accepting, cancels in-flight connections, removes any Unix socket
file it created, and exits with status 0.

**Unix socket files.** If the socket path exists from a previous run and
nothing is listening on it, the stale file is replaced. A live socket or a
non-socket file at that path is an error. Missing parent directories are
created.

**Socket activation.** When `LISTEN_PID`/`LISTEN_FDS` (systemd socket
activation) are set for this process, the server uses an inherited socket
instead of binding. See [Socket activation](#socket-activation-and-fd-inheritance).

## Library usage

All servers implement `EchoServerTrait`:

- `run()` binds and serves until shutdown.
- `shutdown_signal()` returns a `broadcast::Sender<()>`. Calling `send(())` on
  it stops the server gracefully. A shutdown sent before `run()` starts is not
  lost.

Every server also has `bind()`. It creates the socket and returns a bound
server. Its `local_addr()` gives the real address, which is useful with port
`0`, and its `serve()` runs the server. All clients implement `EchoClient`
(`echo(&[u8])` and `echo_string(&str)`).

The library does not install signal handlers. To stop on Ctrl-C, forward the
signal to `shutdown_signal()` yourself.

### TCP

```rust
use echosrv::{EchoClient, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::time::Duration;

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    let config = TcpConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(), // port 0: pick a free port
        max_connections: 100,
        read_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    // TcpEchoServer is StreamEchoServer<TcpProtocol>; it takes a StreamConfig.
    let server = TcpEchoServer::new(config.into());
    let shutdown = server.shutdown_signal();

    let bound = server.bind().await?;
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    let mut client = TcpEchoClient::connect(addr).await?;
    assert_eq!(client.echo_string("hello").await?, "hello");

    shutdown.send(()).unwrap();
    handle.await.unwrap()?;
    Ok(())
}
```

`TcpEchoClient` is the generic stream client `Client<TcpProtocol>`. Use
`connect_with_config` with a `ClientConfig` (or `ClientConfigBuilder`) to set
the connect, read and write timeouts and the maximum response size.

### UDP

```rust
use echosrv::{EchoClient, EchoServerTrait, UdpConfig, UdpEchoClient, UdpEchoServer};

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    // Defaults: 127.0.0.1:0 and a 64 KiB datagram buffer.
    let server = UdpEchoServer::new(UdpConfig::default().into());
    let shutdown = server.shutdown_signal();

    let bound = server.bind().await?;
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    let mut client = UdpEchoClient::connect(addr).await?;
    assert_eq!(client.echo(b"ping").await?, b"ping");

    shutdown.send(()).unwrap();
    handle.await.unwrap()?;
    Ok(())
}
```

Each datagram is echoed to its sender. Datagrams larger than `buffer_size` are
truncated. UDP has no connections, so there is no connection limit.

### HTTP

```rust
use echosrv::{EchoClient, EchoServerTrait, HttpConfig, HttpEchoClient, HttpEchoServer};

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    let server = HttpEchoServer::new(HttpConfig {
        max_body_size: 64 * 1024,
        ..HttpConfig::default() // 127.0.0.1:0
    });
    let shutdown = server.shutdown_signal();

    let bound = server.bind().await?;
    let addr = *bound.local_addr().as_network().unwrap();
    let handle = tokio::spawn(bound.serve());

    // Sends `POST /` with Content-Length; fails on non-2xx responses.
    let mut client = HttpEchoClient::connect(addr).await?;
    assert_eq!(client.echo(b"hello").await?, b"hello");

    shutdown.send(()).unwrap();
    handle.await.unwrap()?;
    Ok(())
}
```

### Unix domain sockets

```rust
use echosrv::unix::{
    UnixDatagramConfig, UnixDatagramEchoClient, UnixDatagramEchoServer, UnixStreamConfig,
    UnixStreamEchoClient, UnixStreamEchoServer,
};
use echosrv::{EchoClient, EchoServerTrait};

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    let dir = tempfile::tempdir().unwrap();

    // Stream
    let path = dir.path().join("stream.sock");
    let server = UnixStreamEchoServer::new(UnixStreamConfig::default().with_socket_path(path.clone()));
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await?.serve());

    let mut client = UnixStreamEchoClient::connect(path.clone()).await?;
    assert_eq!(client.echo_string("hello").await?, "hello");
    drop(client);
    shutdown.send(()).unwrap();
    handle.await.unwrap()?;
    assert!(!path.exists()); // the server removes the socket file it created

    // Datagram
    let path = dir.path().join("dgram.sock");
    let server =
        UnixDatagramEchoServer::new(UnixDatagramConfig::default().with_socket_path(path.clone()));
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await?.serve());

    // The client binds a temporary socket path for replies; it is removed on drop.
    let mut client = UnixDatagramEchoClient::connect(path).await?;
    assert_eq!(client.echo(b"ping").await?, b"ping");

    shutdown.send(()).unwrap();
    handle.await.unwrap()?;
    Ok(())
}
```

### Running until Ctrl-C

```rust,no_run
use echosrv::{EchoServerTrait, TcpConfig, TcpEchoServer};

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    let server = TcpEchoServer::new(
        TcpConfig {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            ..Default::default()
        }
        .into(),
    );
    let shutdown = server.shutdown_signal();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown.send(());
    });
    server.run().await
}
```

### Configuration

| Config               | Fields (defaults)                                                                                                                                          |
|----------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `TcpConfig`          | `bind_addr` (127.0.0.1:0), `max_connections` (100), `buffer_size` (1024), `read_timeout`/`write_timeout` (30 s), `bind_strategy` (None), `service_name` ("tcp"), `rate_limit`/`accept_rate_limit` (None) |
| `UdpConfig`          | `bind_addr` (127.0.0.1:0), `buffer_size` (64 KiB), `read_timeout`/`write_timeout` (30 s), `bind_strategy`, `service_name` ("udp"), `rate_limit` (None) |
| `HttpConfig`         | as TCP plus `buffer_size` (8192), `server_name` ("EchoServer/1.0"), `default_content_type` ("text/plain"), `max_body_size` (1 MiB), `service_name` ("http") |
| `UnixStreamConfig`   | `bind_strategy` (bind `/tmp/echosrv_stream.sock`), `max_connections` (100), `buffer_size` (1024), timeouts (30 s), `service_name` ("unix-stream"), `rate_limit`/`accept_rate_limit` (None) |
| `UnixDatagramConfig` | `bind_strategy` (bind `/tmp/echosrv_datagram.sock`), `buffer_size` (64 KiB), timeouts (30 s), `service_name` ("unix-datagram"), `rate_limit` (None) |

Build configs with `..Default::default()` so new fields do not break your
code. `read_timeout` closes idle stream connections. When `max_connections`
connections are active, new stream connections are accepted and closed right
away.

### Rate limiting

`rate_limit` caps the request rate of a whole server and `accept_rate_limit`
(stream servers only) its rate of new connections. Both take a
`RateLimitConfig { rate_per_sec, burst }` and can be set with
`with_rate_limit` / `with_accept_rate_limit`. Traffic over a limit is rejected,
not delayed:

| Server             | Over `rate_limit`                          | Over `accept_rate_limit`           |
|--------------------|--------------------------------------------|------------------------------------|
| TCP                | connection reset (RST)                     | connection reset (RST)             |
| HTTP               | `429 Too Many Requests` with `Retry-After` | request read, then the same `429`  |
| Unix stream        | connection closed                          | connection closed                  |
| UDP, Unix datagram | datagram dropped                           | n/a                                |

For TCP and Unix streams every chunk read counts as a request; for HTTP every
request does. `Retry-After` is in whole seconds, rounded up, at least 1. The
servers count rejections in `ServerStats` (`stats()` on every server) and log
them at `debug` level only.

```rust
use echosrv::{EchoServerTrait, RateLimitConfig, UdpConfig, UdpEchoServer};

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    // 500 datagrams/s sustained, bursts of up to 50.
    let config = UdpConfig::default().with_rate_limit(RateLimitConfig::new(500, 50));
    let server = UdpEchoServer::new(config.into());
    let stats = server.stats();
    let shutdown = server.shutdown_signal();
    let serving = tokio::spawn(server.bind().await?.serve());

    // ... later
    println!("dropped so far: {}", stats.dropped_rate_limited());
    shutdown.send(()).expect("server is running");
    serving.await.expect("server task panicked")
}
```

The `rate_limit` module also has the primitives: `Gcra`, a lock-free policer
that admits an event or returns how long to wait, and `TokenBucket`, an async
shaper whose `acquire()` waits for a token.

## HTTP semantics

The HTTP server implements a small, strict subset of HTTP/1.1:

- **One request per connection.** Every response carries `Connection: close`.
  There is no keep-alive or pipelining.
- **Only `POST` is accepted.** Other methods get `405 Method Not Allowed` with
  `Allow: POST`. The path is ignored.
- **Framing is by `Content-Length` only.** A request without one has an empty
  body. An empty `POST` still gets `200 OK` with `Content-Length: 0`.
- **Responses.** `200 OK` contains the request body byte for byte. Its headers
  are `Content-Length`, `Connection: close`, plus `Server` and `Content-Type`
  from `HttpConfig` (each is omitted if set to `None`). The body is streamed
  back as it is read, so it is never fully buffered.
- **`Expect: 100-continue`** is answered with `100 Continue` before the body is
  read.
- **Errors.** Error responses have a short `text/plain` body.

  | Status | Cause                                                                            |
  |--------|----------------------------------------------------------------------------------|
  | 400    | Malformed request, more than 32 headers, or invalid/conflicting `Content-Length` |
  | 405    | Method other than `POST`                                                         |
  | 413    | `Content-Length` greater than `max_body_size` (default 1 MiB)                    |
  | 429    | Over the server's `rate_limit` or `accept_rate_limit` (with `Retry-After`)      |
  | 431    | Request line plus headers larger than 8 KiB                                      |
  | 501    | Any `Transfer-Encoding` header (chunked bodies are not supported)                |

```bash
curl -sS --data-binary @payload.bin http://127.0.0.1:8080/ -o echoed.bin
printf 'POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello' | nc 127.0.0.1 8080
```

## Socket activation and FD inheritance

A server can use a listening socket created by another process instead of
binding its own. This works for every protocol, including Unix sockets. Use it
for systemd socket activation, privileged ports, or handing a socket to a new
process during a restart.

Socket acquisition is controlled by `bind_strategy`:

- `None` (network configs) binds `bind_addr`.
- `BindStrategy::Bind(BindTarget)` binds the given address or path.
- `BindStrategy::Inherit(InheritedFd)` must use the given descriptor.
- `BindStrategy::InheritOrBind { fd, fallback_target }` tries the explicit `fd`
  first. Next it takes a descriptor from the systemd pool: the one whose name
  equals the config's `service_name`, or else the only descriptor passed. If
  neither exists, it binds `fallback_target`. When `fallback_target` is `None`
  on a network config, it falls back to `bind_addr`.

`with_fd_inheritance` sets up `InheritOrBind` for you:

```rust
use echosrv::unix::UnixStreamConfig;
use echosrv::{HttpConfig, TcpConfig};

// Use the systemd socket named "http" (or the only socket passed), else bind bind_addr.
let http = HttpConfig::default().with_fd_inheritance("http");
let tcp = TcpConfig::default().with_fd_inheritance("tcp");
// Unix configs need an explicit fallback path.
let unix = UnixStreamConfig::default()
    .with_fd_inheritance("unix-stream".to_string(), "/run/echo.sock".into());
# let _ = (http, tcp, unix);
```

Inherited descriptors are checked before use. They must be sockets of the right
type (stream or datagram) and family (IP or Unix), and stream sockets must
already be listening. Each descriptor is owned by exactly one server. The
systemd environment is parsed once per process. `LISTEN_PID` must match the
current process, and the descriptors are marked close-on-exec. A server never
deletes the socket file of an inherited Unix socket.

Passing a descriptor directly, for example one received from a custom process
manager:

```rust
use echosrv::network::{BindStrategy, InheritedFd};
use echosrv::{EchoClient, EchoServerTrait, TcpConfig, TcpEchoClient, TcpEchoServer};
use std::os::fd::OwnedFd;

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    // Stands in for a listening socket created by a parent process.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    let server = TcpEchoServer::new(
        TcpConfig {
            bind_strategy: Some(BindStrategy::Inherit(InheritedFd::new(OwnedFd::from(listener)))),
            ..Default::default()
        }
        .into(),
    );
    let shutdown = server.shutdown_signal();
    let handle = tokio::spawn(server.bind().await?.serve());

    let mut client = TcpEchoClient::connect(addr).await?;
    assert_eq!(client.echo(b"inherited").await?, b"inherited");

    shutdown.send(()).unwrap();
    handle.await.unwrap()?;
    Ok(())
}
```

If you only have a raw descriptor number, use the `unsafe`
`InheritedFd::from_raw_fd`. You must guarantee that nothing else owns the
descriptor.

### systemd example

The `echosrv` binary always prefers an inherited socket. It looks for the
socket whose `FileDescriptorName=` matches the protocol name (`tcp`, `udp`,
`http`, `unix-stream`, `unix-datagram`). If there is no such socket but exactly
one socket was passed, it uses that one. The port or path argument is only used
when nothing was inherited.

```ini
# /etc/systemd/system/echosrv-http.socket
[Unit]
Description=echosrv HTTP socket

[Socket]
ListenStream=0.0.0.0:8080
FileDescriptorName=http

[Install]
WantedBy=sockets.target
```

```ini
# /etc/systemd/system/echosrv-http.service
[Unit]
Description=echosrv HTTP echo server
Requires=echosrv-http.socket
After=echosrv-http.socket

[Service]
ExecStart=/usr/local/bin/echosrv http
Environment=RUST_LOG=echosrv=info
DynamicUser=yes
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now echosrv-http.socket
curl --data-binary hello http://127.0.0.1:8080/
```

For UDP, use `ListenDatagram=` and `echosrv udp`. For Unix sockets, use
`ListenStream=/run/echosrv.sock` with `echosrv unix-stream`, or
`ListenDatagram=/run/echosrv.sock` with `echosrv unix-dgram`. Do not set
`Accept=yes`: the server needs the listening socket, not individual
connections. systemd keeps the socket open while the service restarts, so
clients queue in the backlog instead of being refused.

## Testing

```bash
cargo test                          # unit, integration and doc tests (README examples included)
cargo test --test tcp               # one integration suite
cargo test --test property_tests    # property-based tests (proptest)
cargo clippy --all-targets
cargo bench                         # Criterion benchmarks (benches/echo_performance.rs)
```

The integration suites are in `tests/`:

| File                       | Covers                                                                |
|----------------------------|-----------------------------------------------------------------------|
| `tests/tcp.rs`             | TCP server and client, connection limits, timeouts, shutdown          |
| `tests/udp.rs`             | UDP server and client                                                 |
| `tests/unix.rs`            | Unix stream and datagram servers, socket file handling                |
| `tests/http.rs`            | HTTP framing, status codes, `HttpEchoClient`                          |
| `tests/rate_limit.rs`      | Request and connection rate limits: 429, resets, drops, counters      |
| `tests/fd_inheritance.rs`  | End-to-end socket inheritance for every protocol                      |
| `tests/cli.rs`             | The `echosrv` binary: arguments, signals, socket activation           |
| `tests/property_tests.rs`  | Echo round trips with random payloads                                 |

Tests bind port `0` or a temporary socket path and get the real address from
`local_addr()`. They use no fixed ports and no sleeps.

## Module layout

```text
src/
├── lib.rs        EchoError, Result, re-exports
├── main.rs       echosrv binary (clap CLI, signals, socket activation)
├── cli_help.rs   help layout for the binary (defaults on their own line in --help)
├── defaults.rs   default protocol, host, port and Unix socket paths
├── rate_limit.rs Gcra, TokenBucket, RateLimitConfig
├── common/       EchoServerTrait, EchoClient, ServerStats, shared server lifecycle
├── stream/       StreamProtocol, StreamEchoServer<P>, Client<P>, StreamConfig
├── datagram/     DatagramProtocol, DatagramEchoServer<P>, DatagramEchoClient<P>, DatagramConfig
├── tcp/          TcpProtocol, TcpConfig, type aliases
├── udp/          UdpProtocol, UdpConfig, type aliases
├── unix/         Unix stream/datagram protocols, servers, clients, configs
├── http/         HttpProtocol (HTTP/1.1 framing), HttpEchoServer, HttpEchoClient, HttpConfig
└── network/      Address, BindStrategy, FdInheritanceConfig, SocketBuilder
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for the architecture and how to add a
protocol.

## License

MIT. See [LICENSE](LICENSE).
