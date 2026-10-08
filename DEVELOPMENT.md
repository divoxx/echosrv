# Development Guide

This guide covers how echosrv is put together, how to add a protocol, and the
testing and release conventions. For user-facing documentation, see
[README.md](README.md) and the rustdoc (`cargo doc --open`).

## Prerequisites

- Rust 1.85 or newer (edition 2024)
- A Unix-like OS. The crate has a `compile_error!` for non-Unix targets.
- Optional: `nc`, `curl` and `socat` for manual testing

```bash
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps
```

## Architecture

### Generic servers over protocol traits

There are two generic servers. Each concrete protocol plugs into one of them by
implementing a trait:

| Generic type                   | Trait              | Implementations                                         |
|--------------------------------|--------------------|---------------------------------------------------------|
| `stream::StreamEchoServer<P>`  | `StreamProtocol`   | `TcpProtocol`, `UnixStreamProtocol`, `HttpProtocol`     |
| `datagram::DatagramEchoServer<P>` | `DatagramProtocol` | `UdpProtocol`, `UnixDatagramProtocol`                |

- **`StreamProtocol`** declares `Listener`, `Stream` and `Error` types. Its
  operations are `bind`, `bind_with_inheritance`, `accept`, `connect`,
  `connect_address`, `read`, `write`, `flush` and `map_io_error`. Listeners
  implement `network::LocalAddress`, so `bind()` can report the real address.
  Rate limiting adds three items with defaults: `reject(stream, reason,
  retry_after)` signals a rejection before the server closes the stream
  (default: nothing, so a plain close; TCP sets `SO_LINGER` 0 to send an RST;
  HTTP answers `429`), and `FRAMED_REQUESTS` / `begin_request` let a protocol
  define what one request is (default: every non-empty read; HTTP: the
  request head).
- **`DatagramProtocol`** declares `Socket`, `PeerAddr` and `Error` types. Its
  operations are `bind`, `bind_with_inheritance`, `recv_from`, `send_to` and
  `map_io_error`. `PeerAddr` is `SocketAddr` for UDP and a socket path for
  Unix datagrams.
- **Clients.** `stream::Client<P>` is the generic stream client. It needs
  `P::Stream: AsyncRead + AsyncWrite + Unpin` because it reads and writes
  concurrently: a large payload must not deadlock on full socket buffers.
  `datagram::DatagramEchoClient<P>` is the datagram client.

The TCP and UDP types are aliases: `TcpEchoServer = StreamEchoServer<TcpProtocol>`,
`TcpEchoClient = Client<TcpProtocol>`, `UdpEchoServer = DatagramEchoServer<UdpProtocol>`.
Their configs (`TcpConfig`, `UdpConfig`) convert into the generic
`StreamConfig`/`DatagramConfig` with `.into()`.

### Unix servers

`UnixStreamEchoServer` and `UnixDatagramEchoServer` are thin structs that
wrap the generic servers. They take Unix-specific configs, which have a
`bind_strategy` instead of a `bind_addr`. The protocols return
`ManagedUnixListener` / `ManagedUnixDatagram`. These wrappers own the socket
file through `unix::socket_file::SocketFile`:

- At bind time, `bind_with_stale_recovery` creates missing parent directories.
  It replaces an existing path only if the path is a socket and connecting to
  it is refused (a stale file). A live socket or a regular file is an error.
- On drop, the file is removed only if it still has the device and inode
  recorded at bind time. Inherited sockets are never unlinked.

`UnixDatagramEchoClient` binds a temporary path so the server can reply, and
removes it on drop. It uses `send_to` rather than `connect`, which avoids
`EISCONN` on macOS.

### HTTP layering

`HttpEchoServer` is a struct that wraps `StreamEchoServer<HttpProtocol>`. The
generic echo loop does not know about HTTP. All HTTP handling is in the stream
type:

- `HttpProtocol::accept` wraps each `TcpStream` in an `HttpStream`. On its
  first `read`, `HttpStream` buffers and parses the request head with
  `httparse`. It enforces the limits (8 KiB head, 32 headers, POST only,
  `Content-Length` framing, `max_body_size`, no `Transfer-Encoding`) and sends
  `100 Continue` when the client asked for it.
- After that, `read` returns body bytes only, and reports EOF at
  `Content-Length`. The first `write` emits the `200 OK` head, and later writes
  stream the echoed body. Rejected requests get their error response, and the
  connection then reads as EOF. Before closing, the server drains leftover
  input briefly (linger) so the client is not sent an RST that would discard
  the response.
- The settings `server_name`, `default_content_type` and `max_body_size` are
  not part of `StreamConfig`. `BoundHttpServer::serve` runs the generic
  `serve()` inside a `tokio::task_local!` scope (`HTTP_SETTINGS`), and
  `accept` reads the settings from it. This works because `accept` runs in the
  serve task itself. Only the per-connection handlers are spawned. A bare
  `StreamEchoServer<HttpProtocol>` falls back to `HttpConfig::default()`.

### Server lifecycle

The lifecycle code is in `common/lifecycle.rs` and the generic `serve()`
loops:

1. **Construction.** `ShutdownSignal::new()` creates the broadcast channel
   *and* its first receiver. A `shutdown_signal().send(())` issued before
   `run()` is therefore not lost.
2. **`bind()`** validates the config, resolves the socket (see below) and
   returns a `'static` `Bound*Server` that holds the socket, its
   `local_addr()` and the shutdown receiver.
3. **`serve()`** runs a `select!` loop (biased toward shutdown) over the
   shutdown signal, finished connection tasks and `accept`. Each connection
   takes a `ConnectionGuard`, an RAII slot that is released even if the task
   panics. Connections over `max_connections` are closed immediately. Tasks
   live in a `JoinSet`. A failed `accept` (for example `EMFILE`) is followed
   by a 100 ms backoff.
   **Rate limits.** Each server builds its `Gcra` limiters (`RateLimiters`)
   once in `new()`, so a limit is global to the server. A connection over
   `accept_rate_limit` keeps its `ConnectionGuard` while a spawned task runs
   `P::reject` (bounded by `REJECT_TIMEOUT`). The connection task checks
   `rate_limit` per request and calls `P::reject` before closing. The datagram
   loop drops over-limit datagrams. All of these increment the shared
   `ServerStats` and log at `debug` only.
4. **Shutdown.** The loop drops the listener (closing the socket and removing
   any owned Unix socket file), cancels a `CancellationToken` that every
   connection task selects on, awaits the `JoinSet`, and returns `Ok(())`.

Signal handling is not done in the library. The binary (`src/main.rs`) maps
`SIGINT` and `SIGTERM` to `shutdown_signal()`.

### FD inheritance flow

```text
systemd env (LISTEN_PID/LISTEN_FDS/LISTEN_FDNAMES)
  └─ FdInheritanceConfig::from_systemd_env()   parsed once per process (OnceLock);
                                               fds marked CLOEXEC; shared take-once pool
config.bind_strategy (+ service_name)
  └─ effective_bind_strategy()                 InheritOrBind without fallback -> bind_addr
     └─ SocketBuilder::resolve_fd()            Bind | Inherit(InheritedFd::take) |
                                               InheritOrBind: explicit fd, else
                                               pool.take_named_or_sole(service_name), else fallback
        └─ BuildSocket::build()                validate_inherited_fd (type, family, listening)
           └─ from_fd() / bind_to()            per-protocol builder (tcp/, udp/, unix/)
```

`InheritedFd` is a cloneable *take-once* handle around an `OwnedFd`, so configs
stay `Clone` while each descriptor has a single owner. A
`BindStrategy::Inherit` whose descriptor was already consumed (for example by
an earlier `run()`) fails with `EchoError::FdInheritance`. `InheritOrBind`
falls back to binding instead. The binary calls `from_systemd_env()` and then
clears the `LISTEN_*` variables before the Tokio runtime starts. At that point
the process has one thread, so changing the environment is safe.

## Adding a protocol

1. Create `src/<proto>/` with a protocol type implementing `StreamProtocol`
   or `DatagramProtocol`. Use `async_trait`, and map I/O errors to a variant of
   `EchoError` (or a custom error type with `Into<EchoError>`).
2. If the protocol should support socket inheritance, implement
   `network::BuildSocket<YourSocket>`. This means setting `SOCKET_TYPE`,
   `VALID_FAMILIES` and `REQUIRE_LISTENING`, and implementing `from_fd` and
   `bind_to`. Call `YourBuilder::build(&config.effective_bind_strategy(),
   &config.service_name, fd_config)` from `bind_with_inheritance`.
3. Add a config struct with `bind_strategy` and `service_name` fields, a
   `Default` impl, `with_fd_inheritance`, and `From<YourConfig> for
   StreamConfig`/`DatagramConfig`.
4. Add type aliases (`pub type FooEchoServer = StreamEchoServer<FooProtocol>;`
   and a client alias), or a wrapper struct if per-server state is needed (see
   `HttpEchoServer`). Re-export them from `lib.rs`.
5. Add unit tests in `src/<proto>/tests.rs` and an integration suite
   `tests/<proto>.rs` with a `start_<proto>` helper in `tests/common/mod.rs`.
6. If the binary should serve it, add it to `Protocol` in `src/main.rs`, then
   update the usage text, the README and `tests/cli.rs`.

## Testing conventions

- **Real servers, real sockets.** Start servers with `bind()` on
  `127.0.0.1:0` or a path inside a `tempfile::TempDir`, then read the address
  from `local_addr()`. The listener exists before the helper returns, so no
  readiness wait is needed.
- **No fixed ports and no sleeps.** Fixed ports collide when tests run in
  parallel. Sleeps are slow and still flaky. Wait on events instead, for
  example `tokio::time::timeout(WAIT, …)` around the thing you expect.
- **Assert graceful shutdown.** `TestServer::stop()` sends shutdown and
  asserts that `serve()` returns `Ok(())` within `WAIT`.
- **Helpers** are in `tests/common/mod.rs`: `start_tcp`, `start_udp`,
  `start_http`, `start_unix_stream`, `start_unix_datagram`, `socket_dir`,
  `payload` and `tagged_payload`. `TestServer::stats` is the server's
  `ServerStats`.
- **CLI tests are serialized** (`serial()` in `tests/cli.rs`). On macOS, std
  sets `FD_CLOEXEC` on a new socket in a separate syscall. A child process
  spawned at the same moment by another test can inherit that socket and keep
  a port or path alive. Tests in this file that spawn processes or create
  sockets therefore hold a global lock.
- **Doc tests.** README code blocks are compiled and run through
  `ReadmeDoctests` in `src/lib.rs`. Mark blocks that bind fixed ports as
  `rust,no_run`, and mark shell or config snippets `bash`/`text`/`ini`.
- Unit tests go in `src/<module>/tests.rs` or inline `#[cfg(test)]` modules.
  Property tests (`tests/property_tests.rs`) reuse one server per test binary.

```bash
cargo test                           # everything, including README doctests
cargo test --test tcp                # tcp | udp | unix | http | rate_limit | fd_inheritance | cli | property_tests
cargo test --lib http::              # HTTP unit tests
cargo test --doc                     # doctests only
```

## Benchmarks

`benches/echo_performance.rs` uses Criterion (`async_tokio`). It starts one
TCP server on port 0 per group and measures echo throughput by payload size,
concurrent clients, and per-round-trip overhead.

```bash
cargo bench                          # HTML reports in target/criterion/
cargo bench --no-run                 # compile check only
```

Compare against a saved baseline (`--save-baseline` / `--baseline`) before
making performance claims.

## Release checklist

1. Make sure `main` is green: `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`,
   `cargo test --doc`, `cargo doc --no-deps` (no warnings), and
   `cargo bench --no-run`.
2. Bump `version` in `Cargo.toml` (breaking changes before 1.0 bump the minor
   version) and run `cargo build` so `Cargo.lock` updates.
3. Move the `CHANGELOG.md` entries from Unreleased to the new version with the
   date, and list breaking changes first.
4. Update the version in the README install snippet.
5. Smoke test the binary: `cargo run -- --help`, `cargo run -- http 8080` plus
   `curl --data-binary hi localhost:8080`, `cargo run -- udp 9090` plus
   `nc -u`, and `cargo run -- unix-stream /tmp/e.sock` plus `nc -U`.
6. `cargo publish --dry-run`, then commit, tag `vX.Y.Z`, push the tag, and
   `cargo publish`.
