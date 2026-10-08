# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

echosrv is a library and CLI of async echo servers and clients (Tokio). It
supports TCP, UDP, HTTP/1.1 (echoes POST bodies), and Unix stream and datagram
sockets. It runs on Unix only. Generic servers sit over protocol traits, and
every protocol supports socket inheritance (systemd socket activation). A
second binary, `echosrv-client`, is a load-testing client for the servers.
README.md is user-facing, and DEVELOPMENT.md has the architecture details.

## Commands

```bash
cargo build
cargo test                               # unit + integration + doctests (README blocks are doctests)
cargo test --test tcp                    # one suite: tcp | udp | unix | http | rate_limit | fd_inheritance | cli | client_cli | property_tests
cargo test --lib http::                  # unit tests of one module
cargo test --doc                         # doctests only
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo bench                              # benches/echo_performance.rs (Criterion)

cargo run -- --help
cargo run -- tcp 8080                    # also: udp 9090 | http | --host 0.0.0.0 tcp
cargo run -- unix-stream /tmp/echo.sock  # also: unix-dgram /tmp/echo_dgram.sock

cargo run --bin echosrv-client -- --help # load-testing client (plain `cargo run` is the server)
cargo test --bin echosrv-client          # client unit tests (cli, runner, stats, report, output)
cargo test --test client_cli             # client black-box tests (spawns the binary)
```

## Module Layout

```
src/
├── lib.rs       EchoError / Result, re-exports, README doctest harness
├── main.rs      CLI: args, RUST_LOG, SIGINT/SIGTERM, LISTEN_FDS socket activation
├── cli/         `cli` feature only, `#[doc(hidden)]`, shared by both binaries: protocol.rs (Protocol names/parsing), target.rs (PORT, HOST:PORT, socket path, --host), logging.rs (init_logging), help.rs (clap help layout, parse helpers), color.rs (--color, NO_COLOR, CLICOLOR_FORCE)
├── defaults.rs  default protocol, host, port, Unix socket paths (server and client)
├── rate_limit.rs Gcra (server policing), TokenBucket (client shaping), RateLimitConfig
├── bin/echosrv-client/   load-testing client binary
│   ├── cli.rs     clap flags, validation and warnings, target resolution, config header
│   ├── runner.rs  workers, payload build/compare, shaper (--rate), conn limiter (--conn-rate), backoff
│   ├── stats.rs   error kinds + classify, Window histograms, OutageTracker, Aggregator, Summary
│   ├── report.rs  text and JSON rendering (config, interval, outage, summary), Verdict
│   ├── output.rs  --color/NO_COLOR/CLICOLOR_FORCE, Palette, [ok]/[fail] tagged lines
│   └── main.rs    SIGINT/SIGTERM and --duration stop gracefully (in-flight requests finish), a second signal aborts; exit codes 0/1/2/130/141/143
├── common/      EchoServerTrait, EchoClient; lifecycle.rs (shutdown signal, ConnectionGuard)
├── stream/      StreamProtocol, StreamEchoServer<P>, BoundStreamServer, Client<P>, StreamConfig
├── datagram/    DatagramProtocol, DatagramEchoServer<P>, DatagramEchoClient<P>, DatagramConfig
├── tcp/         TcpProtocol, TcpConfig, socket builder, type aliases
├── udp/         UdpProtocol, UdpConfig, socket builder, type aliases
├── unix/        Unix stream/datagram protocols, wrapper servers, clients, configs, socket_file.rs
├── http/        HttpProtocol/HttpStream (HTTP/1.1 framing), HttpEchoServer, HttpEchoClient, HttpConfig
└── network/     Address, BindStrategy/InheritedFd/FdInheritanceConfig, SocketBuilder, LocalAddress
tests/           tcp.rs udp.rs unix.rs http.rs rate_limit.rs fd_inheritance.rs cli.rs client_cli.rs
                 property_tests.rs common/mod.rs
benches/         echo_performance.rs
```

## Key Design Points

- `TcpEchoServer = StreamEchoServer<TcpProtocol>`, `UdpEchoServer =
  DatagramEchoServer<UdpProtocol>`. Their configs convert with `.into()`.
  `HttpEchoServer` and the Unix servers are structs that wrap the generic
  servers.
- Servers: `bind()` returns a bound server with `local_addr()` and `serve()`.
  `run()` is the same as `bind().await?.serve().await`. `shutdown_signal()`
  stops the server gracefully, even if sent before `run()`. The library does
  not handle signals; the binary does.
- Config structs have `bind_strategy` and `service_name` fields, so struct
  literals need `..Default::default()`.
- HTTP is POST-only, framed by Content-Length, with one request per connection
  (`Connection: close`). It answers 400, 405, 413, 431 and 501 as documented
  in `src/http/mod.rs`.
- Errors: the library uses `echosrv::Result<T>` / `EchoError`, and the binary
  uses `color-eyre`.
- `echosrv-client` uses only the public library API (the `EchoClient`
  clients, `TokenBucket`, `EchoError::io_error_kind()`/`is_rate_limited()`/
  `retry_after()`). Workers send `Sample`s over an mpsc channel to one
  `Aggregator`, which emits interval/outage events and the `Summary`. Its
  user docs are the README section "Load testing client".

## Testing Conventions

- Start real servers with `bind()` on `127.0.0.1:0` or a `tempfile` socket
  path, then read the address from `local_addr()`. Use the helpers in
  `tests/common/mod.rs` (`start_tcp`, `start_http`, `start_unix_stream`, ...).
- Use no fixed ports and no `sleep`s for readiness. Bound waits with
  `tokio::time::timeout(WAIT, ...)`.
- Assert graceful shutdown (`TestServer::stop()`).
- Tests in `tests/cli.rs` and `tests/client_cli.rs` hold a global `serial()`
  lock. On macOS, a child process spawned concurrently can inherit another
  test's sockets.
- Load safety (client): never run unthrottled load, `--conn-rate unlimited`,
  or the suites in a loop. Every closed TCP connection holds a local port in
  TIME_WAIT for 30-60s; a few hundred new connections/s sustained exhausts
  the ephemeral ports and stalls networking for the whole machine. Keep
  per-request and HTTP runs under `--conn-rate`, keep runs short (`-n` or a
  short `-d`), and rely on the client defaults (100 new connections/s,
  100-200ms backoff) in tests to keep TIME_WAIT low.
- README Rust code blocks must compile and pass as doctests. Use `rust,no_run`
  for fixed ports, and `bash`/`text`/`ini` for non-Rust blocks.
