# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

echosrv is a library and CLI of async echo servers and clients (Tokio). It
supports TCP, UDP, HTTP/1.1 (echoes POST bodies), and Unix stream and datagram
sockets. It runs on Unix only. Generic servers sit over protocol traits, and
every protocol supports socket inheritance (systemd socket activation).
README.md is user-facing, and DEVELOPMENT.md has the architecture details.

## Commands

```bash
cargo build
cargo test                               # unit + integration + doctests (README blocks are doctests)
cargo test --test tcp                    # one suite: tcp | udp | unix | http | fd_inheritance | cli | property_tests
cargo test --lib http::                  # unit tests of one module
cargo test --doc                         # doctests only
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo bench                              # benches/echo_performance.rs (Criterion)

cargo run -- --help
cargo run -- tcp 8080                    # also: udp 9090 | http | --host 0.0.0.0 tcp
cargo run -- unix-stream /tmp/echo.sock  # also: unix-dgram /tmp/echo_dgram.sock
```

## Module Layout

```
src/
├── lib.rs       EchoError / Result, re-exports, README doctest harness
├── main.rs      CLI: args, RUST_LOG, SIGINT/SIGTERM, LISTEN_FDS socket activation
├── common/      EchoServerTrait, EchoClient; lifecycle.rs (shutdown signal, ConnectionGuard)
├── stream/      StreamProtocol, StreamEchoServer<P>, BoundStreamServer, Client<P>, StreamConfig
├── datagram/    DatagramProtocol, DatagramEchoServer<P>, DatagramEchoClient<P>, DatagramConfig
├── tcp/         TcpProtocol, TcpConfig, socket builder, type aliases
├── udp/         UdpProtocol, UdpConfig, socket builder, type aliases
├── unix/        Unix stream/datagram protocols, wrapper servers, clients, configs, socket_file.rs
├── http/        HttpProtocol/HttpStream (HTTP/1.1 framing), HttpEchoServer, HttpEchoClient, HttpConfig
└── network/     Address, BindStrategy/InheritedFd/FdInheritanceConfig, SocketBuilder, LocalAddress
tests/           tcp.rs udp.rs unix.rs http.rs fd_inheritance.rs cli.rs property_tests.rs common/mod.rs
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

## Testing Conventions

- Start real servers with `bind()` on `127.0.0.1:0` or a `tempfile` socket
  path, then read the address from `local_addr()`. Use the helpers in
  `tests/common/mod.rs` (`start_tcp`, `start_http`, `start_unix_stream`, ...).
- Use no fixed ports and no `sleep`s for readiness. Bound waits with
  `tokio::time::timeout(WAIT, ...)`.
- Assert graceful shutdown (`TestServer::stop()`).
- Tests in `tests/cli.rs` hold a global `serial()` lock. On macOS, a child
  process spawned concurrently can inherit another test's sockets.
- README Rust code blocks must compile and pass as doctests. Use `rust,no_run`
  for fixed ports, and `bash`/`text`/`ini` for non-Rust blocks.
