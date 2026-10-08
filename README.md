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

Command-line tools (the `echosrv` server and the
[`echosrv-client`](#load-testing-client-echosrv-client) load tester):

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
created. A Unix stream server also holds an advisory lock on `<path>.lock`
while it runs (removed on shutdown), so a second server never replaces the
socket of a live one, even when that server is too busy to accept.

**Socket activation.** When `LISTEN_PID`/`LISTEN_FDS` (systemd socket
activation) are set for this process, the server uses an inherited socket
instead of binding. See [Socket activation](#socket-activation-and-fd-inheritance).

## Load testing client (`echosrv-client`)

`echosrv-client` is a second binary in the crate (`cargo install echosrv`
installs both). It sends echo requests to an echosrv server from many
concurrent workers, checks every echo byte for byte, and reports throughput,
latency, errors by kind and outages. Use it to stress test a server, to check
its rate limits, or to verify that a restart (for example a systemd socket
activation handover) loses no requests.

```text
Usage: echosrv-client [OPTIONS] [PROTOCOL] [TARGET]
```

From a checkout, run it with `cargo run --bin echosrv-client -- <args>`
(plain `cargo run` still runs the server). `echosrv-client --help` lists every
option with its default, and `-h` is a short summary.

### Targets

The defaults match the server's, so `echosrv-client` with no arguments talks
to what plain `echosrv` serves: TCP on `127.0.0.1:8080`. For `tcp`, `udp` and
`http` the target is `HOST:PORT` or a bare port (host `127.0.0.1`). For the
Unix protocols it is a socket path, by default `/tmp/echosrv_stream.sock`
(`unix-stream`) or `/tmp/echosrv_datagram.sock` (`unix-dgram`, also accepted as
`unix-datagram`).

```bash
echosrv-client                           # tcp 127.0.0.1:8080, like `echosrv`
echosrv-client udp 9090                  # udp 127.0.0.1:9090
echosrv-client http 10.0.0.5:8080        # http to another host
echosrv-client unix-stream               # /tmp/echosrv_stream.sock
echosrv-client unix-datagram /tmp/echo_dgram.sock
```

### Running a test

**Length.** `-n N` sends N requests in total and stops. Without `-n` the run is
continuous until `-d`/`--duration` (`10s`, `500ms`, `2m`) runs out or you press
Ctrl-C. With both, whichever comes first stops the run.

**Stopping.** The end of `--duration` and the first `SIGINT` (Ctrl-C) or
`SIGTERM` all stop the run gracefully: no new requests start, requests already
in flight finish (each bounded by `--timeout`) and are counted, their
connections are closed cleanly, and the summary is printed. So a client
stopped by a supervisor or `kill` never cuts a request off mid-flight, which
the server would otherwise see as a reset. A second signal aborts immediately,
with exit status 130 (`SIGINT`) or 143 (`SIGTERM`).

**Workers.** `-c C` runs C workers in parallel (default 1). Each worker sends
one request, waits for the echo, then sends the next.

**Connections.** `--conn-mode persistent` (the default, except for HTTP) gives
each worker one connection that it reuses until an error, then reconnects.
`--conn-mode per-request` opens a new connection (or datagram socket) for
every request. HTTP is always per-request, because the server closes every
connection after one response; `--conn-mode persistent` with `http` is a usage
error. The two modes answer different questions during a restart. Persistent
connections show what happens to clients that are already connected: whether
their connections are reset and how long until they can reconnect.
Per-request connections show whether *new* clients are turned away at any
moment. With socket activation the listening socket stays open, so a
per-request run should see no `connect_refused` at all, only slower requests
while the new process starts.

**Payloads.** Every payload starts with a unique header,
`echosrv:<worker>:<seq>:`, followed by filler up to `-s`/`--payload-size`
bytes (default 64, grown to fit the header). The filler is a fixed pattern by
default, the text given with `--payload TEXT` (repeated or truncated to the
size; without `-s` the payload is the header plus the text once), or random
bytes with `--random`. The client compares each echo with what it sent, byte
for byte. Any difference is a `mismatch`, and a single mismatch fails the run.
For HTTP, payloads above the server's 1 MiB body limit get a warning, because
the server answers them with `413`.

**Traffic shaping.** `-r`/`--rate RPS` limits the request rate of all workers
together with a token bucket of capacity `-b`/`--burst` (default 1, for smooth
pacing). Workers wait for a token; requests are delayed, never dropped. Above
about 500 req/s, a bucket of 1 cannot keep up with the ~1 ms timer
resolution, so use a burst of about RPS/100 (the client warns otherwise).
Without `--rate` the workers send as fast as the server answers.

**Timeouts.** `-t`/`--timeout` (default `5s`) bounds every connect, read and
write. For UDP and Unix datagrams a lost datagram is only noticed when the
timeout expires, so use a shorter one there (for example `-t 200ms`).

### Connection safety

Every closed TCP connection keeps its local port in `TIME_WAIT` for about 30
seconds on macOS and 60 seconds on Linux, and the ephemeral port range has
only about 16,000 (macOS) to 28,000 (Linux) ports. Above roughly 500 new
connections per second sustained, the machine runs out of local ports, and
networking then stalls for every application on it, not just the test.
`echosrv-client` therefore protects the machine it runs on:

- **`--conn-rate PER_SEC`** caps new connections per second across all
  workers. The default is 100. It applies to every request in per-request mode
  and for HTTP, and to reconnects in persistent mode. The cap allows a burst of
  one connection per worker, so all workers can connect at start. Per-request
  and HTTP runs therefore make at most 100 requests per second unless you
  raise it. `--conn-rate unlimited` lifts the cap. For `tcp` and `http`, values
  above 400 and `unlimited` print a warning.
- **Backoff after errors.** After an error a worker pauses for
  `--reconnect-delay` (default 100 ms) before the next attempt. The pause
  doubles with each further error in a row, up to `--max-backoff` (default
  200 ms), and resets after a success. A server that is down or rejecting
  requests is not hit by a tight reconnect loop. The backoff also bounds how
  late the end of an outage is noticed: at most `--max-backoff` after the
  server is back.
- **Stop on port exhaustion.** If a connect fails with `EADDRNOTAVAIL`, the
  machine is out of local ports. The client records a `ports_exhausted` error,
  stops the whole run at once (retrying would only keep the ports in use),
  prints a hint on stderr and exits with status 1.

Persistent connections (the default for everything but HTTP) open only one
connection per worker, so they are the safe way to push high request rates.

### Output

Before the run starts, the client prints its configuration. Values that came
from defaults are marked `(default)`, so a saved log records exactly what ran.
While running, it prints one line per `-i`/`--interval` (default `1s`;
`-i 0` turns live output off). At the end it prints a summary. A fixed run of
`echosrv-client tcp 18090 -n 10000 -c 8` is fast enough to need no interval
line:

```text
--- echosrv-client ---
target      tcp 127.0.0.1:18090
workers     8, persistent connections (default)
requests    10000
rate        unshaped (default), at most 100 new connections/s (default)
payload     64 bytes (default), pattern filler (default)
timeout     5s (default), backoff 100ms (default) up to 200ms (default)
interval    1s (default)
max errors  0% (default)

--- echosrv-client summary ---
target      tcp 127.0.0.1:18090 (persistent, c=8)
elapsed     0.12s (completed)
requests    10000 of 10000 total, 10000 ok, 0 errors (0.00%)
throughput  81929.4 req/s (81929.4 ok/s)
latency     min=0.03ms p50=0.09ms p90=0.14ms p99=0.20ms p99.9=0.27ms max=0.35ms mean=0.09ms
outages     none
  [ok] no mismatches, error rate 0.00% within --max-error-rate 0%
```

**Interval lines** show the elapsed time, attempts per second, successes,
errors (with a breakdown by kind when there are any) and the p50 and p99
latency of the successful requests in that interval (`-` if none succeeded).
`OUTAGE` at the end means an outage was still open when the interval ended.

**Outages.** An outage starts with the first error that means the server is
unavailable and ends with the next success, counted across all workers. The
client prints a line when one starts (with the kind of the first error) and
when it ends (with its length and error count). A success only ends an outage
if the request *started* after the outage did: requests already in flight on
other connections can still complete after the server has gone, and they say
nothing about whether it is back.

**Summary.** The summary gives the stop reason (`completed`, `duration` or
`interrupt`, plus `interrupted before -n completed` when `-n` was not
reached), totals, throughput, errors by kind, latency percentiles of the
successful requests and every outage (the first 10 are listed). Its last line
is the verdict, `[ok]` or `[fail]` with the reason, which matches the exit
status.

### Testing a graceful restart

Start a server, then a continuous client with a few workers and a short
interval:

```bash
echosrv tcp 18090                                # terminal 1
echosrv-client tcp 18090 -c 4 -i 500ms -d 5s     # terminal 2
```

Then stop the server (Ctrl-C) and start it again. Here it was stopped about
1.6 s into the run and restarted about 1.5 s later (configuration header
omitted):

```text
[    0.5s] 59606 req/s ok=29841 err=0 p50=0.06ms p99=0.12ms
[    1.0s] 61224 req/s ok=30611 err=0 p50=0.06ms p99=0.11ms
[    1.5s] 60938 req/s ok=30469 err=0 p50=0.06ms p99=0.11ms
[    1.6s] [fail] outage started (reset)
[    2.0s] 11460 req/s ok=5741 err=12 (connect_refused=8 reset=4) p50=0.06ms p99=0.11ms OUTAGE
[    2.5s] 16 req/s ok=0 err=8 (connect_refused=8) p50=- p99=- OUTAGE
[    3.0s] 24 req/s ok=0 err=12 (connect_refused=12) p50=- p99=- OUTAGE
[    3.1s]   [ok] outage ended after 1.52s (32 errors)
[    3.5s] 45669 req/s ok=22791 err=0 p50=0.06ms p99=0.12ms
[    4.0s] 61598 req/s ok=30800 err=0 p50=0.06ms p99=0.11ms
--- echosrv-client summary ---
target      tcp 127.0.0.1:18090 (persistent, c=4)
elapsed     5.00s (duration)
requests    210811 total, 210779 ok, 32 errors (0.02%)
throughput  42153.7 req/s (42147.3 ok/s)
errors      connect_refused=28 reset=4
latency     min=0.02ms p50=0.06ms p90=0.09ms p99=0.12ms p99.9=0.15ms max=0.42ms mean=0.06ms
outages     1 (total 1.52s, longest 1.52s)
            at    1.59s for 1.52s (32 errors)
[fail] error rate 0.02% above --max-error-rate 0%
```

The shutdown reset the four open connections (`reset=4`). While the server was
down, each worker retried every 100–200 ms and was refused, which is why the
rate drops to a few attempts per second instead of spinning. The first success
after the restart ended the outage. A restart that loses nothing shows
`outages none`. To accept a known number of errors, raise `--max-error-rate`.
Run the same test with `--conn-mode per-request` to see what new clients
experience during the restart.

### Error kinds

| Kind              | Meaning                                                                            | Outage |
|-------------------|------------------------------------------------------------------------------------|--------|
| `connect_refused` | Connection refused: nothing listening on the port, or a stale Unix socket file    | yes    |
| `connect_failed`  | Any other connect failure, including a missing Unix socket path                    | yes    |
| `reset`           | Connection reset, aborted or closed before the whole echo arrived                  | yes    |
| `timeout`         | Connect, read or write took longer than `--timeout` (also a lost datagram)         | yes    |
| `rate_limited`    | The server rejected the request with HTTP `429 Too Many Requests`                  | no     |
| `mismatch`        | The echo differs from the payload sent (always fails the run)                      | yes    |
| `ports_exhausted` | This machine ran out of local ports (`EADDRNOTAVAIL`); the run stops               | no     |
| `other`           | Anything else                                                                      | yes    |

`rate_limited` is an answer from a live server and `ports_exhausted` is a
problem of the client machine, so neither opens an outage. Both still count
as errors for `--max-error-rate`. Only HTTP can say "rate limited": the TCP
server resets over-limit connections and the Unix stream server closes them,
which shows up as `reset`, and datagram servers drop the datagram, which
shows up as `timeout`. See [Rate limiting](#rate-limiting).

### Rate-limited servers and `Retry-After`

Against `echosrv http 18091 --rate 50 --burst 5`, four workers get about 50
successes per second and the rest is answered with `429`
(`echosrv-client http 18091 -c 4 -d 3s`; header omitted):

```text
[    1.0s] 91 req/s ok=54 err=37 (rate_limited=37) p50=0.19ms p99=0.52ms
[    2.0s] 83 req/s ok=50 err=33 (rate_limited=33) p50=0.23ms p99=0.89ms
[    3.0s] 87 req/s ok=50 err=37 (rate_limited=37) p50=0.24ms p99=2.02ms
--- echosrv-client summary ---
target      http 127.0.0.1:18091 (per-request, c=4)
elapsed     3.00s (duration)
requests    261 total, 154 ok, 107 errors (41.00%)
throughput  86.9 req/s (51.3 ok/s)
errors      rate_limited=107
latency     min=0.10ms p50=0.22ms p90=0.35ms p99=0.89ms p99.9=2.02ms max=2.02ms mean=0.25ms
outages     none
[fail] error rate 41.00% above --max-error-rate 0%
```

By default a worker treats a `429` like any other error and retries after the
normal backoff. With `--honor-retry-after`, it waits for the server's
`Retry-After` delay instead (whole seconds, at least 1), as a well-behaved
client would. The flag only affects HTTP. To match the server's limit from
the client side instead, shape the traffic with `--rate`.

### JSON output

`--json` replaces the text on stdout with one JSON object per line. The first
line is the configuration and the last is the summary; interval and outage
lines come in between, as they happen. Warnings and diagnostics stay on
stderr.

```bash
echosrv-client tcp 18090 -n 200 -c 2 --json | jq -c 'select(.type == "summary")'
```

```text
{"type":"config","protocol":"tcp","target":"127.0.0.1:18090","concurrency":2,"conn_mode":"persistent","requests":200,"duration_s":null,"rate":null,"burst":null,"conn_rate":100,"payload_size":64,"filler":"pattern","timeout_s":5.0,"reconnect_delay_s":0.1,"max_backoff_s":0.2,"honor_retry_after":false,"interval_s":1.0,"max_error_rate_pct":0.0,"defaults":[...]}
```

| `type`         | Fields                                                                                                                                                                                                 |
|----------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `config`       | `protocol`, `target`, `concurrency`, `conn_mode`, `requests`, `duration_s`, `rate`, `burst`, `conn_rate`, `payload_size`, `filler` (`pattern`/`text`/`random`), `timeout_s`, `reconnect_delay_s`, `max_backoff_s`, `honor_retry_after`, `interval_s`, `max_error_rate_pct`, `defaults` (names of the fields that came from defaults). `null` means unlimited, unshaped or off. |
| `interval`     | `elapsed_s`, `interval_s`, `req_per_sec`, `total`, `ok`, `errors`, `errors_by_kind` (non-zero kinds only), `p50_ms`, `p99_ms` (`null` without successes), `outage_open`                               |
| `outage_start` | `elapsed_s`, `kind` (the first error)                                                                                                                                                                  |
| `outage_end`   | `elapsed_s`, `start_s`, `duration_ms`, `errors`                                                                                                                                                        |
| `summary`      | `protocol`, `target`, `concurrency`, `conn_mode`, `requests`, `elapsed_s`, `total`, `ok`, `errors`, `errors_by_kind` (every kind), `error_rate_pct`, `req_per_sec`, `ok_per_sec`, `latency`, `outages`, `interrupted`, `stop_reason` |

In the summary, `latency` is `null` when nothing succeeded, and otherwise has
`min_ms`, `mean_ms`, `p50_ms`, `p90_ms`, `p99_ms`, `p999_ms` and `max_ms`.
`outages` has `count`, `total_ms`, `longest_ms`, `ongoing` (an outage was
still open at the end), `windows_truncated` (more than 100 outages) and
`windows`, a list of `{start_s, duration_ms, errors, ongoing}`. An outage still
open at the end has no `outage_end` line; it appears in `windows` with
`ongoing: true`. `stop_reason` is `completed`, `duration`, `interrupt`
(`SIGINT`), `terminated` (`SIGTERM`) or `ports_exhausted`. The verdict is not in the JSON; use the exit status.

### Exit status

| Status | Meaning                                                                                      |
|--------|----------------------------------------------------------------------------------------------|
| 0      | No mismatches and the error rate is within `--max-error-rate` (default 0%)                   |
| 1      | An echo mismatch, an error rate above `--max-error-rate`, or the machine ran out of ports     |
| 2      | Usage or setup error (bad flags, unresolvable host, `--conn-mode persistent` with `http`)    |
| 130    | Aborted by a second `SIGINT` (Ctrl-C)                                                        |
| 143    | Aborted by a second `SIGTERM`                                                                |
| 141    | stdout was closed (for example by `\| head`); the run stops quietly                          |

The error rate is errors divided by attempts, including failed connects and
`rate_limited` answers. `--max-error-rate 0.5` allows 0.5%.

### Colors and logging

`--color auto` (the default) colors the text report on stdout and the
diagnostics on stderr, each only if that stream is a terminal. `NO_COLOR`
turns color off and `CLICOLOR_FORCE` turns it on even when piped.
`--color always` and `--color never` override both. JSON is never colored.
Warnings and failures go to stderr as `[warn]` and `[fail]` lines. `-v` turns
on debug logs (each failed request, with its error), and `RUST_LOG` overrides
the log filter.

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

### Client errors

Clients never return a partial echo as success. Every client has connect (stream
clients), read and write timeouts and a buffer size: `ClientConfig` for TCP,
HTTP and Unix stream, `DatagramClientConfig` for UDP and Unix datagram (both
with `connect_with_config`). Failures are reported as:

| Situation                                        | Error                                                    |
|--------------------------------------------------|----------------------------------------------------------|
| Nothing listening on the port                    | `Tcp` I/O error, kind `ConnectionRefused`                |
| No Unix socket at the path / stale socket file   | `Unix` I/O error, kind `NotFound` / `ConnectionRefused`  |
| Connection reset (e.g. TCP rate limit)           | I/O error, kind `ConnectionReset` (or `BrokenPipe`)      |
| Connection closed before the whole echo arrived  | I/O error, kind `UnexpectedEof`                          |
| Connect, read or write took too long             | `Timeout` (also a datagram that was dropped)             |
| Reply larger than the client's limit             | `Config` (stream: `max_response_size`, datagram: `buffer_size`) |
| HTTP response with a non-2xx status              | `HttpStatus { status, reason, retry_after, body }`       |

`EchoError::io_error_kind()` returns the `std::io::ErrorKind` of the I/O
variants (`Tcp`, `Udp`, `Unix`), `is_rate_limited()` is true for an HTTP `429`,
and `retry_after()` returns its `Retry-After` delay:

```rust
use echosrv::{EchoClient, EchoServerTrait, HttpConfig, HttpEchoClient, HttpEchoServer};
use echosrv::RateLimitConfig;

#[tokio::main]
async fn main() -> echosrv::Result<()> {
    let config = HttpConfig::default().with_rate_limit(RateLimitConfig::new(1, 1));
    let bound = HttpEchoServer::new(config).bind().await?;
    let addr = *bound.local_addr().as_network().unwrap();
    tokio::spawn(bound.serve());

    let mut client = HttpEchoClient::connect(addr).await?;
    client.echo(b"first").await?;
    match client.echo(b"second").await {
        Err(e) if e.is_rate_limited() => println!("429, retry in {:?}", e.retry_after()),
        Err(e) => println!("failed: {e} (kind {:?})", e.io_error_kind()),
        Ok(_) => println!("admitted"),
    }
    Ok(())
}
```

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
`ListenDatagram=/run/echosrv.sock` with `echosrv unix-dgram`. On Linux,
abstract sockets (`ListenStream=@echosrv`) work too; the server reports their
address as `unix:@echosrv`. Do not set
`Accept=yes`: the server needs the listening socket, not individual
connections. systemd keeps the socket open while the service restarts, so
clients queue in the backlog instead of being refused.

## Testing

```bash
cargo test                          # unit, integration and doc tests (README examples included)
cargo test --test tcp               # one integration suite
cargo test --test property_tests    # property-based tests (proptest)
cargo test --test client_cli        # the echosrv-client binary (light, throttled runs)
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
| `tests/client_cli.rs`      | The `echosrv-client` binary: flags, output, outages, exit codes       |
| `tests/property_tests.rs`  | Echo round trips with random payloads                                 |

Tests bind port `0` or a temporary socket path and get the real address from
`local_addr()`. They use no fixed ports and no sleeps.

## Module layout

```text
src/
├── lib.rs        EchoError, Result, re-exports
├── main.rs       echosrv binary (clap CLI, signals, socket activation)
├── bin/echosrv-client/  echosrv-client binary (load testing, see above)
├── cli_help.rs   help layout shared by both binaries (defaults on their own line in --help)
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
