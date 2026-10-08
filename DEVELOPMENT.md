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
  HTTP answers `429`, or `503` for `RejectReason::TooManyConnections`), and `FRAMED_REQUESTS` / `begin_request` let a protocol
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
  It replaces an existing path only if the path is a socket and a
  non-blocking connect to it is refused (a stale file). A live socket or a
  regular file is an error. The probe never blocks, so binding inside
  `async fn bind` cannot stall a runtime thread on a full backlog.
- Stream sockets also take an exclusive `flock` on the sidecar `<path>.lock`
  and hold it in `SocketFile` until the socket file is removed. macOS and the
  BSDs refuse a connect to a listener with a full backlog with the same
  `ECONNREFUSED` as a dead socket, so the probe alone could unlink an
  overloaded server's socket; the lock settles it, and also serializes
  recovery between servers starting together. Listeners that do not take the
  lock (other programs) are still judged by the probe alone. If the lock file
  cannot be opened or locked, binding falls back to the probe with a warning.
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
   panics. A connection over `max_connections` is rejected with
   `RejectReason::TooManyConnections` and counted in
   `ServerStats::rejected_over_capacity`. Tasks
   live in a `JoinSet`. A failed `accept` (for example `EMFILE`) is followed
   by a 100 ms backoff. The datagram loop backs off the same way after a
   failed `recv_from`, selecting on shutdown during the wait, and logs the
   first and every 100th consecutive failure at `error`.
   **Rate limits.** Each server builds its `Gcra` limiters (`RateLimiters`)
   once in `new()`, so a limit is global to the server. A connection over
   `accept_rate_limit` (or over `max_connections`) is handed to a spawned task
   that runs `P::reject` (bounded by `REJECT_TIMEOUT`). These tasks do not
   hold a `max_connections` slot; they take one of `MAX_PENDING_REJECTIONS`
   (32) slots of their own, and a connection rejected while all are busy is
   closed without `P::reject`, so a flood cannot lock out admitted clients. The connection task checks
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

### Load-testing client

`echosrv-client` (`src/bin/echosrv-client/`) is a separate binary built only
on the public library API, so it also exercises that API the way users see
it. It shares the `Protocol` enum (names, aliases, parsing), target parsing,
logging setup, the help layout and color resolution with the server through
the library's `cli` module (`src/cli/`, `#[doc(hidden)]`, built only with the
`cli` feature), and takes its default targets from `src/defaults.rs`, so both
binaries agree on them.

```text
cli.rs      Cli (clap) ── resolve() ──> RunConfig + warnings
header.rs   RunConfig ──> RunInfo (shared by header and summary), RunHeader (config line);
            defaulted(ArgMatches) -> the HeaderFields left at their defaults
runner.rs   C workers ─┬─ shaper: TokenBucket (--rate/--burst), shared
                       ├─ conn limiter: TokenBucket (--conn-rate), before every new client
                       ├─ make_client() -> Box<dyn EchoClient>, echo, byte-for-byte compare
                       └─ backoff after errors (--reconnect-delay doubling to --max-backoff)
                │ Sample { at, latency, outcome }   (bounded mpsc channel)
                v
stats.rs    Aggregator ── interval Window (hdrhistogram) ──> LiveEvent::Interval every -i
                       ── OutageTracker ──────────────────> LiveEvent::Outage (start/end)
                       └─ on channel close ───────────────> RunStats
runner.rs   Summary::new(RunInfo, RunStats, StopReason)
report.rs   text or JSON lines for each event; Verdict from the Summary
main.rs     SIGINT/SIGTERM stop the run (CancellationToken: no new attempts, in-flight
            attempts finish); a second signal exits 130/143; verdict -> exit code
```

- **Workers** take sequence numbers from a shared counter (so `-n` is exact
  across workers), build the `echosrv:<worker>:<seq>:` payload and compare the
  echo. Every error drops the client; persistent mode reconnects through the
  connection limiter, per-request mode always does.
- **Classification** (`stats::classify`) maps `EchoError` to an `ErrorKind`
  through `is_rate_limited()`, `Timeout` and `io_error_kind()`.
  `ErrorKind::is_outage()` is true only for `connect_refused`,
  `connect_failed`, `reset` and `timeout`. A
  `ports_exhausted` error cancels the whole run and sets the
  `ports_exhausted` stop reason.
- **Stop reason.** `runner::run` takes the `CancellationToken` and an
  `Arc<OnceLock<StopReason>>`; whoever stops the run sets the reason first
  (`runner::stop`), and the first reason set wins. The signal handler in
  `main.rs` sets `interrupt` / `terminated`, the runner's own `--duration`
  timer sets `duration`, a worker sets `ports_exhausted`, and a run nobody
  stopped is `completed`.
- **Outages** are tracked by the single aggregator, so they are global across
  workers. A success closes an outage only if the attempt started after the
  outage did; samples arrive slightly out of order and in-flight requests can
  still finish after the server has gone.
- **Output** is one line per event on stdout (text or JSON), written through
  `emit()`, which exits with 141 when stdout is closed. Diagnostics are tagged
  `[info]`/`[warn]`/`[fail]` lines on stderr. Color is decided per stream in
  `output.rs`.
- **Load safety.** The `--conn-rate` default (100/s) and the error backoff
  exist because a load test can otherwise exhaust the machine's ephemeral
  ports through TIME_WAIT. Keep both defaults conservative.

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
6. Add a variant to the shared `Protocol` enum in `src/cli/protocol.rs` (its
   doc comment is the help text of both binaries; `as_str` and
   `service_name` give its names) and to `Target::default_for` in
   `src/cli/target.rs`. The compiler then points at the exhaustive matches
   to extend: `start()` in `src/main.rs`, and `Cli::resolve` and `Transport`
   in the client, with a branch in `runner::make_client`. Update the README,
   `tests/cli.rs` and `tests/client_cli.rs`. Options with a literal default
   use clap's `default_value`; optional or computed defaults are written at
   the end of the doc comment as ` [default: …]`, which `src/cli/help.rs`
   moves onto its own line in `--help`. Shared default endpoints live in
   `src/defaults.rs`.

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
- **CLI tests are serialized** (`serial()` in `tests/cli.rs`,
  `tests/client_cli.rs` and `tests/client_signals.rs`). On macOS, std
  sets `FD_CLOEXEC` on a new socket in a separate syscall. A child process
  spawned at the same moment by another test can inherit that socket and keep
  a port or path alive. Tests in this file that spawn processes or create
  sockets therefore hold a global lock.
- **Doc tests.** README code blocks are compiled and run through
  `ReadmeDoctests` in `src/lib.rs`. Mark blocks that bind fixed ports as
  `rust,no_run`, and mark shell or config snippets `bash`/`text`/`ini`.
- **Client tests stay light.** `tests/client_cli.rs` and
  `tests/client_signals.rs` (helpers in `tests/client_common/mod.rs`) run
  the client against in-process servers with small `-n` or short `-d` and the default
  `--conn-rate`. Do not add unthrottled runs (`--conn-rate unlimited`, high
  per-request rates) and do not loop the suites: TIME_WAIT sockets from a few
  hundred new connections/s can exhaust the machine's ephemeral ports.
- **Keep the connection count low.** Every closed TCP connection leaves a
  TIME_WAIT socket behind for 30-60s, and the suites run in the same window.
  A test should open well under a couple of hundred new connections: reuse a
  client across checks where the order does not matter instead of connecting
  per iteration or per generated case.
- Unit tests go in `src/<module>/tests.rs` or inline `#[cfg(test)]` modules.
  Property tests (`tests/property_tests.rs`) reuse one server per test binary
  and pool their TCP clients, so a property opens a handful of connections
  rather than one per case.

```bash
cargo test                           # everything, including README doctests
cargo test --test tcp                # tcp | udp | unix | http | rate_limit | fd_inheritance | cli | client_cli | client_signals | property_tests
cargo test --bin echosrv-client      # client unit tests
cargo test --lib http::              # HTTP unit tests
cargo test --doc                     # doctests only
```

## Benchmarks

`benches/echo_performance.rs` uses Criterion (`async_tokio`). It starts one
TCP server on port 0 per group and measures echo throughput by payload size,
concurrent clients, and per-round-trip overhead.

Each benchmark connects its clients once, before measuring, and reuses them
for every iteration (`iter_custom`), so the measurements exclude connection
setup and a whole run opens a few dozen connections. Keep it that way:
Criterion runs tens of thousands of iterations, and a connection per
iteration leaves that many ports in TIME_WAIT, which can exhaust the
ephemeral port range. Connection setup is not benchmarked, because
Criterion's time-based warm-up cannot bound the number of connections.

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
