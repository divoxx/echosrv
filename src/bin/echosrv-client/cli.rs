//! Command-line interface: flags, validation and target resolution.

use crate::report::RunHeader;
use crate::runner::{
    ConnMode, DEFAULT_CONN_RATE, DEFAULT_PAYLOAD_SIZE, Filler, RunConfig, SAFE_CONN_RATE, Transport,
};
use clap::parser::ValueSource;
use clap::{ArgMatches, Parser};
use echosrv::RateLimitConfig;
use echosrv::cli::color::ColorChoice;
use echosrv::cli::{Protocol, Target};
use echosrv::defaults::DEFAULT_PROTOCOL;
use echosrv::http::DEFAULT_MAX_BODY_SIZE;
use std::ffi::OsStr;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

/// Token bucket capacity without `--burst`: smooth pacing.
pub const DEFAULT_BURST: u32 = 1;

/// Highest `--rate` (req/s) that a bucket smaller than rate/100 can pace
/// smoothly: above it each wait is close to the ~1ms timer resolution and
/// wake-up overshoot is lost to the capacity cap. This is about timer
/// resolution only; it is unrelated to port exhaustion ([`SAFE_CONN_RATE`]).
const SMOOTH_PACING_MAX_RATE: u32 = 500;

/// A rate limit that may be lifted: `unlimited` or a positive number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit(pub Option<u32>);

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(n) => write!(f, "{n}"),
            None => f.write_str("unlimited"),
        }
    }
}

fn parse_limit(s: &str) -> Result<Limit, String> {
    if s.eq_ignore_ascii_case("unlimited") {
        return Ok(Limit(None));
    }
    match s.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Limit(Some(n))),
        _ => Err(format!(
            "expected a positive number or `unlimited`, got {s:?}"
        )),
    }
}

/// Parses durations like `10s`, `500ms`, `2m`, `1h`, `250us`, `1.5s`.
/// A bare number means seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    if num.is_empty() {
        return Err(format!("invalid duration {s:?}: missing number"));
    }
    let value: f64 = num
        .parse()
        .map_err(|_| format!("invalid duration {s:?}: bad number {num:?}"))?;
    let secs_per_unit = match unit.trim() {
        "" | "s" | "sec" | "secs" => 1.0,
        "ms" => 1e-3,
        "us" | "µs" => 1e-6,
        "ns" => 1e-9,
        "m" | "min" | "mins" => 60.0,
        "h" | "hr" | "hrs" => 3600.0,
        other => {
            return Err(format!(
                "invalid duration {s:?}: unknown unit {other:?} (use ns, us, ms, s, m, h)"
            ));
        }
    };
    Duration::try_from_secs_f64(value * secs_per_unit)
        .map_err(|e| format!("invalid duration {s:?}: {e}"))
}

fn parse_pct(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .trim_end_matches('%')
        .parse()
        .map_err(|_| format!("invalid percentage {s:?}"))?;
    if (0.0..=100.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("percentage {v} must be between 0 and 100"))
    }
}

/// Load / stress-test client for echosrv servers.
///
/// Runs N requests (`-n`) or continuously (until Ctrl-C or `--duration`) with
/// C concurrent workers, optionally shaped by a token bucket, and reports
/// throughput, latency, error classes and outage windows.
#[derive(Debug, Parser)]
#[command(
    name = "echosrv-client",
    version,
    after_help = "Examples:\n  \
        echosrv-client                             tcp to 127.0.0.1:8080 (what plain `echosrv` serves)\n  \
        echosrv-client -n 10000 -c 50              10000 requests over 50 workers\n  \
        echosrv-client -c 10 -i 500ms              continuous; restart the server and watch for outages\n  \
        echosrv-client udp                         udp to 127.0.0.1:8080 (`echosrv udp`)\n  \
        echosrv-client udp 9090 -n 1000 --json     udp to 127.0.0.1:9090, JSON lines on stdout\n  \
        echosrv-client unix-stream -d 5s -c 4      /tmp/echosrv_stream.sock (`echosrv unix-stream`)\n  \
        echosrv-client tcp 10.0.0.5:8080 -c 20 --rate 500 --burst 50 -d 10s\n  \
        echosrv-client http 8081 -c 16 -d 5s --honor-retry-after\n\n\
        Exit codes: 0 ok, 1 mismatch, error rate above --max-error-rate or local\n\
        ports exhausted, 2 usage/setup error, 130 / 143 aborted by a second\n\
        SIGINT (Ctrl-C) / SIGTERM, 141 stdout closed.\n\n\
        The first SIGINT or SIGTERM (and the end of --duration) stops gracefully:\n\
        no new requests start, requests in flight finish and the summary is printed."
)]
pub struct Cli {
    /// Protocol to speak.
    #[arg(value_name = "PROTOCOL", default_value = DEFAULT_PROTOCOL, value_parser = Protocol::parser())]
    pub protocol: Protocol,

    /// HOST:PORT or a bare PORT (host 127.0.0.1) for tcp/udp/http, or a socket
    /// path for unix-stream/unix-dgram; matches the `echosrv` server defaults
    /// [default: 127.0.0.1:8080 for tcp/udp/http, /tmp/echosrv_stream.sock for
    /// unix-stream, /tmp/echosrv_datagram.sock for unix-dgram].
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,

    /// Total requests (attempts, including failed connects)
    /// [default: unlimited — runs until Ctrl-C or --duration].
    #[arg(short = 'n', long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub requests: Option<u64>,

    /// Number of concurrent workers.
    #[arg(
        short,
        long,
        value_name = "C",
        default_value_t = 1,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..10_000)
    )]
    pub concurrency: usize,

    /// Stop after this long (e.g. 10s, 500ms, 2m); with -n, whichever comes
    /// first [default: none].
    #[arg(short, long, value_name = "DUR", value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// Global request rate (req/s) across all workers; blocks to pace traffic
    /// [default: unshaped].
    #[arg(short, long, value_name = "RPS", value_parser = clap::value_parser!(u32).range(1..))]
    pub rate: Option<u32>,

    /// Token bucket capacity (1 = smooth pacing; use about RPS/100 above ~500 req/s) [default: 1].
    #[arg(short, long, value_name = "N", requires = "rate", value_parser = clap::value_parser!(u32).range(1..))]
    pub burst: Option<u32>,

    /// Payload size in bytes, grown to fit the `echosrv:<worker>:<seq>:` header
    /// [default: 64, or header + TEXT with --payload].
    #[arg(short = 's', long, value_name = "B")]
    pub payload_size: Option<usize>,

    /// Filler text after the sequence header (repeated/truncated to
    /// --payload-size) [default: a fixed byte pattern].
    #[arg(long, value_name = "TEXT", conflicts_with = "random")]
    pub payload: Option<String>,

    /// Random filler bytes instead of a fixed pattern [default: off].
    #[arg(long)]
    pub random: bool,

    /// Connect/read/write timeout.
    #[arg(short, long, value_name = "DUR", default_value = "5s", value_parser = parse_duration)]
    pub timeout: Duration,

    /// Connection handling [default: persistent; per-request for http].
    #[arg(long, value_enum, value_name = "MODE")]
    pub conn_mode: Option<ConnMode>,

    /// Maximum new connections per second across all workers (every
    /// request in per-request mode and for http, reconnects otherwise), or
    /// `unlimited`. Closed connections hold a local port for 30-60s, so
    /// above about 400/s the machine can run out of ports.
    #[arg(
        long,
        value_name = "PER_SEC",
        default_value_t = Limit(Some(DEFAULT_CONN_RATE)),
        value_parser = parse_limit
    )]
    pub conn_rate: Limit,

    /// Pause after an error before the next attempt; it doubles with each
    /// further error in a row, up to --max-backoff.
    #[arg(long, value_name = "DUR", default_value = "100ms", value_parser = parse_duration)]
    pub reconnect_delay: Duration,

    /// Longest pause between attempts while errors continue. It also bounds
    /// how late the end of an outage is noticed.
    #[arg(long, value_name = "DUR", default_value = "200ms", value_parser = parse_duration)]
    pub max_backoff: Duration,

    /// On HTTP 429, sleep for the server's Retry-After before the next
    /// request [default: off — the normal error backoff].
    #[arg(long)]
    pub honor_retry_after: bool,

    /// Live stats interval; 0 disables live output.
    #[arg(short, long, value_name = "DUR", default_value = "1s", value_parser = parse_duration)]
    pub interval: Duration,

    /// Emit JSON lines on stdout (config, intervals, outage events, summary); never
    /// colored [default: off — human-readable text].
    #[arg(long)]
    pub json: bool,

    /// When to color human output; stdout and stderr are decided separately
    /// and JSON is never colored.
    #[arg(long, value_enum, value_name = "WHEN", default_value = "auto")]
    pub color: ColorChoice,

    /// Maximum error rate (percent of attempts, rate-limited included) for exit code 0.
    #[arg(long, value_name = "PCT", default_value_t = 0.0, value_parser = parse_pct)]
    pub max_error_rate: f64,

    /// Verbose logging (debug) on stderr [default: off — warnings only;
    /// RUST_LOG overrides].
    #[arg(short, long)]
    pub verbose: bool,
}

/// A validated, resolved configuration plus the warnings found on the way.
#[derive(Debug)]
pub struct Validated {
    pub config: RunConfig,
    pub warnings: Vec<String>,
}

impl Cli {
    /// Checks flag combinations that clap cannot express and returns the
    /// effective connection mode and [target](Self::target).
    pub fn check(&self) -> Result<(ConnMode, Target), String> {
        if self.protocol == Protocol::Http && self.conn_mode == Some(ConnMode::Persistent) {
            return Err(
                "--conn-mode persistent is not supported for http (the server closes every connection after one response)"
                    .into(),
            );
        }
        let target = self.target()?;
        if self.payload.as_deref() == Some("") {
            return Err("--payload must not be empty".into());
        }
        let conn_mode = self
            .conn_mode
            .unwrap_or(if self.protocol == Protocol::Http {
                ConnMode::PerRequest
            } else {
                ConnMode::Persistent
            });
        Ok((conn_mode, target))
    }

    /// The effective target: the per-protocol default when omitted, and a
    /// bare port meaning `127.0.0.1:PORT` for tcp/udp/http.
    pub fn target(&self) -> Result<Target, String> {
        match self.target.as_deref() {
            Some(value) => Target::parse_connect(self.protocol, OsStr::new(value)),
            None => Ok(Target::default_for(self.protocol)),
        }
    }

    fn filler(&self) -> Filler {
        match (&self.payload, self.random) {
            (Some(text), _) => Filler::Text(text.clone().into_bytes()),
            (None, true) => Filler::Random,
            (None, false) => Filler::Pattern,
        }
    }

    /// Validates the flags and resolves the target into a [`RunConfig`].
    pub async fn resolve(&self) -> Result<Validated, String> {
        let (conn_mode, target) = self.check()?;
        let mut warnings = Vec::new();

        let transport = match self.protocol {
            Protocol::Tcp => Transport::Tcp(resolve_addr(&target).await?),
            Protocol::Udp => Transport::Udp(resolve_addr(&target).await?),
            Protocol::Http => Transport::Http(resolve_addr(&target).await?),
            // A Unix target is its socket path.
            Protocol::UnixStream => Transport::UnixStream(target.to_string().into()),
            Protocol::UnixDatagram => Transport::UnixDgram(target.to_string().into()),
        };

        let filler = self.filler();
        // Without -s, a --payload TEXT is sent as header + TEXT.
        let payload_size = match (self.payload_size, &filler) {
            (Some(size), _) => Some(size),
            (None, Filler::Text(_)) => None,
            (None, _) => Some(DEFAULT_PAYLOAD_SIZE),
        };

        let is_http = self.protocol == Protocol::Http;
        if let Some(size) = payload_size.filter(|&size| is_http && size > DEFAULT_MAX_BODY_SIZE) {
            warnings.push(format!(
                "http payload of {size} bytes is over the echosrv server's {DEFAULT_MAX_BODY_SIZE}-byte body limit; it answers 413 to larger bodies"
            ));
        }
        let burst = self.burst.unwrap_or(DEFAULT_BURST);
        if let Some(rate) = self
            .rate
            .filter(|&rate| rate > SMOOTH_PACING_MAX_RATE && burst < rate / 100)
        {
            warnings.push(format!(
                "--rate {rate} with --burst {burst}: the ~1ms timer resolution caps smooth pacing well below the target; use --burst {} or more",
                rate / 100
            ));
        }
        let uses_tcp = matches!(self.protocol, Protocol::Tcp | Protocol::Http);
        if uses_tcp && self.conn_rate.0.is_none_or(|n| n > SAFE_CONN_RATE) {
            warnings.push(format!(
                "--conn-rate {}: closed connections hold a local port for 30-60s; above ~{SAFE_CONN_RATE} new connections/s this machine can run out of ports, which stalls networking for every application (the run stops if that happens)",
                self.conn_rate
            ));
        }
        if self.reconnect_delay > self.max_backoff {
            warnings.push(format!(
                "--reconnect-delay is above --max-backoff; pauses are capped at {:?}",
                self.max_backoff
            ));
        }
        if self.honor_retry_after && self.protocol != Protocol::Http {
            warnings.push("--honor-retry-after only has an effect for http".into());
        }

        Ok(Validated {
            config: RunConfig {
                transport,
                requests: self.requests,
                concurrency: self.concurrency,
                rate: self.rate.map(|rate| RateLimitConfig::new(rate, burst)),
                payload_size,
                filler,
                timeout: self.timeout,
                conn_mode,
                // Bursts let every worker open its first connection at once.
                conn_rate: self.conn_rate.0.map(|rate| {
                    let workers = u32::try_from(self.concurrency).unwrap_or(u32::MAX);
                    RateLimitConfig::new(rate, workers.clamp(1, rate))
                }),
                reconnect_delay: self.reconnect_delay,
                max_backoff: self.max_backoff,
                honor_retry_after: self.honor_retry_after,
                interval: (!self.interval.is_zero()).then_some(self.interval),
            },
            warnings,
        })
    }
}

/// Resolves a `HOST:PORT` target to its first address.
async fn resolve_addr(target: &Target) -> Result<SocketAddr, String> {
    let target = target.to_string();
    tokio::net::lookup_host(target.as_str())
        .await
        .map_err(|e| format!("cannot resolve {target:?}: {e}"))?
        .next()
        .ok_or_else(|| format!("{target:?} resolved to no addresses"))
}

/// Whether the argument `id` was left at its default (not given on the
/// command line).
fn is_default(matches: &ArgMatches, id: &str) -> bool {
    matches.value_source(id) != Some(ValueSource::CommandLine)
}

impl Cli {
    /// The configuration header for a resolved run. `matches` must come
    /// from the same parse as `self`; it tells given values from defaults.
    pub fn header(&self, matches: &ArgMatches, config: &RunConfig) -> RunHeader {
        // (argument id, header field) pairs.
        const FIELDS: [(&str, &str); 15] = [
            ("protocol", "protocol"),
            ("target", "target"),
            ("concurrency", "concurrency"),
            ("conn_mode", "conn_mode"),
            ("requests", "requests"),
            ("duration", "duration_s"),
            ("rate", "rate"),
            ("burst", "burst"),
            ("payload_size", "payload_size"),
            ("timeout", "timeout_s"),
            ("conn_rate", "conn_rate"),
            ("reconnect_delay", "reconnect_delay_s"),
            ("max_backoff", "max_backoff_s"),
            ("honor_retry_after", "honor_retry_after"),
            ("interval", "interval_s"),
        ];
        let mut defaults: Vec<&'static str> = FIELDS
            .iter()
            .filter(|(id, _)| is_default(matches, id))
            .map(|(_, field)| *field)
            .collect();
        if is_default(matches, "payload") && is_default(matches, "random") {
            defaults.push("filler");
        }
        if is_default(matches, "max_error_rate") {
            defaults.push("max_error_rate_pct");
        }
        RunHeader {
            kind: "config",
            protocol: config.transport.protocol().as_str(),
            target: config.transport.to_string(),
            concurrency: config.concurrency,
            conn_mode: config.conn_mode.as_str(),
            requests: config.requests,
            duration_s: self.duration,
            rate: config.rate.map(|r| r.rate_per_sec),
            burst: config.rate.map(|r| r.burst),
            conn_rate: config.conn_rate.map(|r| r.rate_per_sec),
            payload_size: config.payload_size,
            filler: config.filler.as_str(),
            timeout_s: config.timeout,
            reconnect_delay_s: config.reconnect_delay,
            max_backoff_s: config.max_backoff,
            honor_retry_after: config.honor_retry_after,
            interval_s: config.interval,
            max_error_rate_pct: self.max_error_rate,
            defaults,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echosrv::defaults::{
        DEFAULT_HOST, DEFAULT_PORT, DEFAULT_UNIX_DGRAM_PATH, DEFAULT_UNIX_STREAM_PATH,
    };

    fn parse_with_matches(args: &[&str]) -> Result<(Cli, ArgMatches), clap::Error> {
        echosrv::cli::help::try_parse_from::<Cli, _, _>(
            std::iter::once("echosrv-client").chain(args.iter().copied()),
        )
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        parse_with_matches(args).map(|(cli, _)| cli)
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("10s"), Ok(Duration::from_secs(10)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
        assert_eq!(parse_duration("250us"), Ok(Duration::from_micros(250)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("3"), Ok(Duration::from_secs(3)));
        assert_eq!(parse_duration("0"), Ok(Duration::ZERO));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("ms").is_err());
        assert!(parse_duration("10x").is_err());
        assert!(parse_duration("1.2.3s").is_err());
        assert!(parse_duration("-1s").is_err());
    }

    fn target(args: &[&str]) -> (Protocol, String) {
        let cli = parse(args).unwrap();
        (cli.protocol, cli.target().unwrap().to_string())
    }

    #[test]
    fn target_defaults_match_server() {
        let tcp = (Protocol::Tcp, "127.0.0.1:8080".to_string());
        assert_eq!(target(&[]), tcp);
        assert_eq!(target(&["-n", "100"]), tcp);
        assert_eq!(target(&["tcp"]), tcp);
        assert_eq!(target(&["udp"]), (Protocol::Udp, "127.0.0.1:8080".into()));
        assert_eq!(target(&["http"]), (Protocol::Http, "127.0.0.1:8080".into()));
        assert_eq!(
            target(&["unix-stream"]),
            (Protocol::UnixStream, DEFAULT_UNIX_STREAM_PATH.into())
        );
        assert_eq!(
            target(&["unix-dgram"]),
            (Protocol::UnixDatagram, "/tmp/echosrv_datagram.sock".into())
        );
        // The server's alias and case-insensitive names work too.
        assert_eq!(
            target(&["unix-datagram"]),
            (Protocol::UnixDatagram, DEFAULT_UNIX_DGRAM_PATH.into())
        );
        assert_eq!(target(&["UDP"]).0, Protocol::Udp);
        assert_eq!(
            target(&["udp", "9090"]),
            (Protocol::Udp, "127.0.0.1:9090".into())
        );
        assert_eq!(
            target(&["tcp", "host:1234"]),
            (Protocol::Tcp, "host:1234".into())
        );
        assert_eq!(
            target(&["unix-stream", "/tmp/x.sock"]),
            (Protocol::UnixStream, "/tmp/x.sock".into())
        );
    }

    #[test]
    fn help_states_defaults() {
        let help = echosrv::cli::help::command::<Cli>()
            .render_long_help()
            .to_string();
        let default_addr = SocketAddr::new(DEFAULT_HOST, DEFAULT_PORT).to_string();
        for needle in [
            default_addr.as_str(),
            DEFAULT_UNIX_STREAM_PATH,
            DEFAULT_UNIX_DGRAM_PATH,
            "[default: tcp]",
            "[default: auto]",
            "[default: unlimited",
            "[default: none]",
            "[default: unshaped]",
            &format!("[default: {DEFAULT_PAYLOAD_SIZE},"),
            // The literal numbers in the flag docs match the constants.
            &format!("above about {SAFE_CONN_RATE}/s"),
            &format!("above ~{SMOOTH_PACING_MAX_RATE} req/s"),
        ] {
            assert!(help.contains(needle), "--help lacks {needle:?}:\n{help}");
        }
        // In --help every default sits on its own line, like clap's built-in ones.
        for line in help.lines() {
            if let Some(at) = line.find("[default: ") {
                assert!(line[..at].trim().is_empty(), "inline default: {line:?}");
            }
        }
    }

    fn conn_mode(cli: &Cli) -> Result<ConnMode, String> {
        cli.check().map(|(mode, _)| mode)
    }

    #[test]
    fn defaults() {
        let cli = parse(&["tcp", "127.0.0.1:8080"]).unwrap();
        assert_eq!(cli.protocol, Protocol::Tcp);
        assert_eq!(cli.color, ColorChoice::Auto);
        assert_eq!(cli.requests, None);
        assert_eq!(cli.concurrency, 1);
        assert_eq!(cli.timeout, Duration::from_secs(5));
        assert_eq!(cli.interval, Duration::from_secs(1));
        assert_eq!(cli.reconnect_delay, Duration::from_millis(100));
        assert_eq!(cli.max_error_rate, 0.0);
        assert_eq!(conn_mode(&cli), Ok(ConnMode::Persistent));
    }

    #[test]
    fn full_flag_set() {
        let cli = parse(&[
            "http",
            "localhost:8081",
            "-n",
            "100",
            "-c",
            "8",
            "-d",
            "2m",
            "-r",
            "50",
            "-b",
            "5",
            "-s",
            "128",
            "--random",
            "-t",
            "250ms",
            "--reconnect-delay",
            "1s",
            "--honor-retry-after",
            "-i",
            "0",
            "--json",
            "--color",
            "never",
            "--max-error-rate",
            "2.5",
            "-v",
        ])
        .unwrap();
        assert_eq!(cli.requests, Some(100));
        assert_eq!(cli.concurrency, 8);
        assert_eq!(cli.duration, Some(Duration::from_secs(120)));
        assert_eq!((cli.rate, cli.burst), (Some(50), Some(5)));
        assert_eq!(cli.payload_size, Some(128));
        assert!(cli.random && cli.json && cli.verbose && cli.honor_retry_after);
        assert_eq!(cli.interval, Duration::ZERO);
        assert_eq!(cli.max_error_rate, 2.5);
        assert_eq!(cli.color, ColorChoice::Never);
        assert_eq!(conn_mode(&cli), Ok(ConnMode::PerRequest));
    }

    #[test]
    fn usage_errors() {
        // Bad protocol (named like the server does), bad --color.
        let err = parse(&["quic", "127.0.0.1:1"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        let err = err.to_string();
        assert!(err.contains("unknown protocol 'quic'"), "{err}");
        assert!(err.contains("unix-dgram"), "{err}");
        assert!(parse(&["--color", "sometimes"]).is_err());
        // Concurrency must be >= 1, -n >= 1.
        assert!(parse(&["tcp", "127.0.0.1:1", "-c", "0"]).is_err());
        assert!(parse(&["tcp", "127.0.0.1:1", "-n", "0"]).is_err());
        // --burst requires --rate.
        let err = parse(&["tcp", "127.0.0.1:1", "--burst", "5"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
        // --payload conflicts with --random.
        assert!(parse(&["tcp", "127.0.0.1:1", "--payload", "x", "--random"]).is_err());
        // Bad durations / percentages.
        assert!(parse(&["tcp", "127.0.0.1:1", "-d", "soon"]).is_err());
        assert!(parse(&["tcp", "127.0.0.1:1", "--max-error-rate", "101"]).is_err());
        // Usage errors exit with code 2.
        assert_eq!(
            parse(&["tcp", "127.0.0.1:1", "-c", "0"])
                .unwrap_err()
                .exit_code(),
            2
        );
    }

    #[test]
    fn semantic_checks() {
        let cli = parse(&["http", "127.0.0.1:1", "--conn-mode", "persistent"]).unwrap();
        assert!(cli.check().unwrap_err().contains("persistent"));

        let cli = parse(&["tcp", "127.0.0.1:1", "--conn-mode", "per-request"]).unwrap();
        assert_eq!(conn_mode(&cli), Ok(ConnMode::PerRequest));

        let cli = parse(&["tcp", "/tmp/echo.sock"]).unwrap();
        assert!(cli.check().unwrap_err().contains("HOST:PORT"));
        let cli = parse(&["udp", "notaport"]).unwrap();
        assert!(cli.check().is_err());
        let cli = parse(&["udp", "70000"]).unwrap();
        assert!(cli.check().is_err());
        let cli = parse(&["tcp", "localhost:http"]).unwrap();
        assert!(cli.check().unwrap_err().contains("HOST:PORT"));
        let cli = parse(&["udp", "8080"]).unwrap();
        assert!(cli.check().is_ok());
        let cli = parse(&["unix-stream", ""]).unwrap();
        assert!(cli.check().unwrap_err().contains("empty"));

        let cli = parse(&["unix-stream", "/tmp/echo.sock"]).unwrap();
        assert_eq!(conn_mode(&cli), Ok(ConnMode::Persistent));
        let cli = parse(&["unix-dgram", "relative.sock"]).unwrap();
        assert!(cli.check().is_ok());
    }

    async fn header_for(args: &[&str]) -> RunHeader {
        let (cli, matches) = parse_with_matches(args).unwrap();
        let config = cli.resolve().await.unwrap().config;
        cli.header(&matches, &config)
    }

    #[tokio::test]
    async fn header_marks_defaults() {
        let h = header_for(&[]).await;
        assert_eq!(h.kind, "config");
        assert_eq!((h.protocol, h.target.as_str()), ("tcp", "127.0.0.1:8080"));
        assert_eq!((h.concurrency, h.conn_mode), (1, "persistent"));
        assert_eq!(h.payload_size, Some(64));
        assert_eq!(h.interval_s, Some(Duration::from_secs(1)));
        assert_eq!(
            h.defaults,
            [
                "protocol",
                "target",
                "concurrency",
                "conn_mode",
                "requests",
                "duration_s",
                "rate",
                "burst",
                "payload_size",
                "timeout_s",
                "conn_rate",
                "reconnect_delay_s",
                "max_backoff_s",
                "honor_retry_after",
                "interval_s",
                "filler",
                "max_error_rate_pct",
            ]
        );

        // Explicit values are not defaults, even when equal to the default.
        let h = header_for(&[
            "tcp",
            "9090",
            "-c",
            "1",
            "-n",
            "10",
            "-d",
            "30s",
            "-r",
            "500",
            "-s",
            "64",
            "--random",
            "-t",
            "5s",
            "-i",
            "0",
            "--max-error-rate",
            "0",
        ])
        .await;
        assert_eq!(h.target, "127.0.0.1:9090");
        assert_eq!(
            (h.requests, h.duration_s),
            (Some(10), Some(Duration::from_secs(30)))
        );
        assert_eq!((h.rate, h.burst), (Some(500), Some(1)));
        assert_eq!((h.filler, h.interval_s), ("random", None));
        assert_eq!(
            h.defaults,
            [
                "conn_mode",
                "burst",
                "conn_rate",
                "reconnect_delay_s",
                "max_backoff_s",
                "honor_retry_after"
            ]
        );

        let h = header_for(&["http", "--payload", "hi", "--honor-retry-after"]).await;
        assert_eq!((h.protocol, h.conn_mode), ("http", "per-request"));
        assert_eq!((h.payload_size, h.filler), (None, "text"));
        assert!(h.honor_retry_after);
        assert!(h.is_default("target") && h.is_default("conn_mode"));
        assert!(!h.is_default("filler") && !h.is_default("protocol"));
    }

    #[tokio::test]
    async fn conn_rate_defaults_and_warnings() {
        let v = parse(&["-c", "8"]).unwrap().resolve().await.unwrap();
        // Bursts let every worker connect at once.
        assert_eq!(
            v.config.conn_rate,
            Some(RateLimitConfig::new(DEFAULT_CONN_RATE, 8))
        );
        assert_eq!(v.config.max_backoff, Duration::from_millis(200));
        assert!(v.warnings.is_empty(), "{:?}", v.warnings);

        let v = parse(&["http", "--conn-rate", "unlimited"])
            .unwrap()
            .resolve()
            .await
            .unwrap();
        assert_eq!(v.config.conn_rate, None);
        let safe = format!("above ~{SAFE_CONN_RATE} new connections/s");
        assert!(
            v.warnings
                .iter()
                .any(|w| w.starts_with("--conn-rate unlimited:") && w.contains(&safe)),
            "{:?}",
            v.warnings
        );

        // The safe rate itself is fine; one more warns.
        let at = SAFE_CONN_RATE.to_string();
        let v = parse(&["tcp", "--conn-rate", &at])
            .unwrap()
            .resolve()
            .await
            .unwrap();
        assert!(v.warnings.is_empty(), "{:?}", v.warnings);
        let over = (SAFE_CONN_RATE + 1).to_string();
        let v = parse(&["tcp", "--conn-rate", &over])
            .unwrap()
            .resolve()
            .await
            .unwrap();
        assert!(
            v.warnings.iter().any(|w| w.contains(&safe)),
            "{:?}",
            v.warnings
        );

        let v = parse(&["tcp", "--conn-rate", "1000", "-c", "2000"])
            .unwrap()
            .resolve()
            .await
            .unwrap();
        assert_eq!(v.config.conn_rate, Some(RateLimitConfig::new(1000, 1000)));
        assert!(v.warnings.iter().any(|w| w.contains("--conn-rate 1000")));

        // Datagram and Unix sockets don't use TCP ports: no warning.
        let v = parse(&["udp", "--conn-rate", "unlimited"])
            .unwrap()
            .resolve()
            .await
            .unwrap();
        assert!(v.warnings.is_empty(), "{:?}", v.warnings);

        assert!(parse(&["--conn-rate", "0"]).is_err());
        assert!(parse(&["--conn-rate", "lots"]).is_err());
    }

    #[tokio::test]
    async fn resolve_builds_run_config() {
        let cli = parse(&["udp", "127.0.0.1:9090", "-r", "100", "-i", "0"]).unwrap();
        let v = cli.resolve().await.unwrap();
        assert!(matches!(v.config.transport, Transport::Udp(a) if a.port() == 9090));
        assert_eq!(v.config.rate, Some(RateLimitConfig::new(100, 1)));
        assert_eq!(v.config.interval, None);
        assert_eq!(v.config.payload_size, Some(64));
        assert!(v.warnings.is_empty());

        let cli = parse(&["tcp", "9091"]).unwrap();
        let v = cli.resolve().await.unwrap();
        assert!(
            matches!(v.config.transport, Transport::Tcp(a) if a.to_string() == "127.0.0.1:9091")
        );

        let cli = parse(&["tcp", "localhost:9090", "--payload", "hello"]).unwrap();
        let v = cli.resolve().await.unwrap();
        assert_eq!(v.config.payload_size, None);
        assert!(matches!(v.config.filler, Filler::Text(ref t) if t == b"hello"));

        // Large HTTP bodies are fine up to the server's body limit.
        let cli = parse(&[
            "http",
            "127.0.0.1:1",
            "-s",
            "1048576",
            "--honor-retry-after",
        ])
        .unwrap();
        assert!(cli.resolve().await.unwrap().warnings.is_empty());
        let cli = parse(&["http", "127.0.0.1:1", "-s", "1048577"]).unwrap();
        let v = cli.resolve().await.unwrap();
        assert_eq!(v.warnings.len(), 1);
        assert!(v.warnings[0].contains("413"), "{:?}", v.warnings);

        let cli = parse(&["tcp", "127.0.0.1:1", "-r", "5000"]).unwrap();
        let v = cli.resolve().await.unwrap();
        assert!(v.warnings[0].contains("--burst 50"), "{:?}", v.warnings);
        let cli = parse(&["tcp", "127.0.0.1:1", "-r", "5000", "-b", "50"]).unwrap();
        assert!(cli.resolve().await.unwrap().warnings.is_empty());

        let cli = parse(&["unix-dgram", "/tmp/x.sock", "--honor-retry-after"]).unwrap();
        let v = cli.resolve().await.unwrap();
        assert!(matches!(v.config.transport, Transport::UnixDgram(_)));
        assert_eq!(v.warnings.len(), 1);

        let cli = parse(&["tcp", "no-such-host.invalid:80"]).unwrap();
        assert!(cli.resolve().await.is_err());
    }
}
