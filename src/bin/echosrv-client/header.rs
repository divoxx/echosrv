//! Run metadata: [`RunInfo`], shared by the configuration header and the
//! summary, and the [`RunHeader`] printed before the first interval.
//!
//! Everything here is derived from the resolved [`RunConfig`]. The only
//! thing taken from the command line is which header fields were left at
//! their defaults ([`defaulted`]).

use crate::runner::RunConfig;
use clap::ArgMatches;
use clap::parser::ValueSource;
use serde::Serialize;
use std::time::Duration;

/// What was run: the fields both the header and the summary start with.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunInfo {
    pub protocol: &'static str,
    /// Resolved address or socket path.
    pub target: String,
    pub concurrency: usize,
    pub conn_mode: &'static str,
    /// Requested number of attempts; `None` = unlimited (continuous).
    pub requests: Option<u64>,
}

impl RunInfo {
    pub fn new(config: &RunConfig) -> Self {
        Self {
            protocol: config.transport.protocol().as_str(),
            target: config.transport.to_string(),
            concurrency: config.concurrency,
            conn_mode: config.conn_mode.as_str(),
            requests: config.requests,
        }
    }
}

/// A field of the [`RunHeader`], named in `defaults` by its JSON key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderField {
    Protocol,
    Target,
    Concurrency,
    ConnMode,
    Requests,
    #[serde(rename = "duration_s")]
    Duration,
    Rate,
    Burst,
    PayloadSize,
    #[serde(rename = "timeout_s")]
    Timeout,
    ConnRate,
    #[serde(rename = "reconnect_delay_s")]
    ReconnectDelay,
    #[serde(rename = "max_backoff_s")]
    MaxBackoff,
    HonorRetryAfter,
    #[serde(rename = "interval_s")]
    Interval,
    Filler,
    #[serde(rename = "max_error_rate_pct")]
    MaxErrorRate,
}

impl HeaderField {
    /// Every field, in the order `defaults` lists them.
    pub const ALL: [HeaderField; 17] = [
        HeaderField::Protocol,
        HeaderField::Target,
        HeaderField::Concurrency,
        HeaderField::ConnMode,
        HeaderField::Requests,
        HeaderField::Duration,
        HeaderField::Rate,
        HeaderField::Burst,
        HeaderField::PayloadSize,
        HeaderField::Timeout,
        HeaderField::ConnRate,
        HeaderField::ReconnectDelay,
        HeaderField::MaxBackoff,
        HeaderField::HonorRetryAfter,
        HeaderField::Interval,
        HeaderField::Filler,
        HeaderField::MaxErrorRate,
    ];

    /// The clap argument ids that set this field; it is defaulted when none
    /// of them was given on the command line.
    fn arg_ids(self) -> &'static [&'static str] {
        match self {
            HeaderField::Protocol => &["protocol"],
            HeaderField::Target => &["target"],
            HeaderField::Concurrency => &["concurrency"],
            HeaderField::ConnMode => &["conn_mode"],
            HeaderField::Requests => &["requests"],
            HeaderField::Duration => &["duration"],
            HeaderField::Rate => &["rate"],
            HeaderField::Burst => &["burst"],
            HeaderField::PayloadSize => &["payload_size"],
            HeaderField::Timeout => &["timeout"],
            HeaderField::ConnRate => &["conn_rate"],
            HeaderField::ReconnectDelay => &["reconnect_delay"],
            HeaderField::MaxBackoff => &["max_backoff"],
            HeaderField::HonorRetryAfter => &["honor_retry_after"],
            HeaderField::Interval => &["interval"],
            HeaderField::Filler => &["payload", "random"],
            HeaderField::MaxErrorRate => &["max_error_rate"],
        }
    }
}

/// The header fields whose values came from defaults rather than the
/// command line. `matches` must come from parsing [`crate::cli::Cli`].
pub fn defaulted(matches: &ArgMatches) -> Vec<HeaderField> {
    HeaderField::ALL
        .into_iter()
        .filter(|f| {
            f.arg_ids()
                .iter()
                .all(|id| matches.value_source(id) != Some(ValueSource::CommandLine))
        })
        .collect()
}

/// The resolved run configuration, printed once before the first interval
/// so the output records what was actually used.
#[derive(Debug, Clone, Serialize)]
pub struct RunHeader {
    #[serde(flatten)]
    pub info: RunInfo,
    /// `None` = no time limit.
    #[serde(serialize_with = "secs::opt")]
    pub duration_s: Option<Duration>,
    /// Client-side shaping in req/s; `None` = unshaped.
    pub rate: Option<u32>,
    /// Token bucket capacity; `None` when unshaped.
    pub burst: Option<u32>,
    /// Cap on new connections per second; `None` = unlimited.
    pub conn_rate: Option<u32>,
    /// `None` = sequence header + the `--payload` text.
    pub payload_size: Option<usize>,
    /// `pattern`, `text` or `random`.
    pub filler: &'static str,
    #[serde(serialize_with = "secs::one")]
    pub timeout_s: Duration,
    #[serde(serialize_with = "secs::one")]
    pub reconnect_delay_s: Duration,
    #[serde(serialize_with = "secs::one")]
    pub max_backoff_s: Duration,
    pub honor_retry_after: bool,
    /// `None` = live interval output disabled (`-i 0`).
    #[serde(serialize_with = "secs::opt")]
    pub interval_s: Option<Duration>,
    pub max_error_rate_pct: f64,
    /// The fields above whose values came from defaults.
    pub defaults: Vec<HeaderField>,
}

/// Serializes `Duration`s as seconds (`f64`), for the `*_s` header fields.
mod secs {
    use serde::Serializer;
    use std::time::Duration;

    pub fn one<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }

    pub fn opt<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => s.serialize_some(&d.as_secs_f64()),
            None => s.serialize_none(),
        }
    }
}

impl RunHeader {
    /// The header for `config`. `max_error_rate_pct` is the pass/fail
    /// threshold, which is not part of the run itself.
    pub fn new(config: &RunConfig, max_error_rate_pct: f64, defaults: Vec<HeaderField>) -> Self {
        Self {
            info: RunInfo::new(config),
            duration_s: config.duration,
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
            max_error_rate_pct,
            defaults,
        }
    }

    pub fn is_default(&self, field: HeaderField) -> bool {
        self.defaults.contains(&field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use crate::runner::Transport;
    use crate::stats::{Aggregator, StopReason, Summary};
    use tokio::time::Instant;

    fn parse(args: &[&str]) -> (Cli, ArgMatches) {
        echosrv::cli::help::try_parse_from::<Cli, _, _>(
            std::iter::once("echosrv-client").chain(args.iter().copied()),
        )
        .unwrap()
    }

    /// The JSON names of `fields`.
    fn names(fields: &[HeaderField]) -> Vec<String> {
        fields
            .iter()
            .map(|f| {
                serde_json::to_value(f)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    async fn header_for(args: &[&str]) -> RunHeader {
        let (cli, matches) = parse(args);
        let config = cli.resolve().await.unwrap().config;
        RunHeader::new(&config, cli.max_error_rate, defaulted(&matches))
    }

    #[tokio::test]
    async fn header_marks_defaults() {
        let h = header_for(&[]).await;
        assert_eq!(
            (h.info.protocol, h.info.target.as_str()),
            ("tcp", "127.0.0.1:8080")
        );
        assert_eq!((h.info.concurrency, h.info.conn_mode), (1, "persistent"));
        assert_eq!(h.payload_size, Some(64));
        assert_eq!(h.interval_s, Some(Duration::from_secs(1)));
        assert_eq!(
            names(&h.defaults),
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
        assert_eq!(h.info.target, "127.0.0.1:9090");
        assert_eq!(
            (h.info.requests, h.duration_s),
            (Some(10), Some(Duration::from_secs(30)))
        );
        assert_eq!((h.rate, h.burst), (Some(500), Some(1)));
        assert_eq!((h.filler, h.interval_s), ("random", None));
        assert_eq!(
            names(&h.defaults),
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
        assert_eq!((h.info.protocol, h.info.conn_mode), ("http", "per-request"));
        assert_eq!((h.payload_size, h.filler), (None, "text"));
        assert!(h.honor_retry_after);
        assert!(h.is_default(HeaderField::Target) && h.is_default(HeaderField::ConnMode));
        assert!(!h.is_default(HeaderField::Filler) && !h.is_default(HeaderField::Protocol));
    }

    #[tokio::test]
    async fn header_json_lines() {
        let cases: [(&[&str], &str); 3] = [
            (
                &[],
                r#"{"type":"config","protocol":"tcp","target":"127.0.0.1:8080","concurrency":1,"conn_mode":"persistent","requests":null,"duration_s":null,"rate":null,"burst":null,"conn_rate":100,"payload_size":64,"filler":"pattern","timeout_s":5.0,"reconnect_delay_s":0.1,"max_backoff_s":0.2,"honor_retry_after":false,"interval_s":1.0,"max_error_rate_pct":0.0,"defaults":["protocol","target","concurrency","conn_mode","requests","duration_s","rate","burst","payload_size","timeout_s","conn_rate","reconnect_delay_s","max_backoff_s","honor_retry_after","interval_s","filler","max_error_rate_pct"]}"#,
            ),
            (
                &[
                    "tcp",
                    "9090",
                    "-c",
                    "3",
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
                ],
                r#"{"type":"config","protocol":"tcp","target":"127.0.0.1:9090","concurrency":3,"conn_mode":"persistent","requests":10,"duration_s":30.0,"rate":500,"burst":1,"conn_rate":100,"payload_size":64,"filler":"random","timeout_s":5.0,"reconnect_delay_s":0.1,"max_backoff_s":0.2,"honor_retry_after":false,"interval_s":null,"max_error_rate_pct":0.0,"defaults":["conn_mode","burst","conn_rate","reconnect_delay_s","max_backoff_s","honor_retry_after"]}"#,
            ),
            (
                &[
                    "http",
                    "--payload",
                    "hi",
                    "--honor-retry-after",
                    "--conn-rate",
                    "unlimited",
                ],
                r#"{"type":"config","protocol":"http","target":"127.0.0.1:8080","concurrency":1,"conn_mode":"per-request","requests":null,"duration_s":null,"rate":null,"burst":null,"conn_rate":null,"payload_size":null,"filler":"text","timeout_s":5.0,"reconnect_delay_s":0.1,"max_backoff_s":0.2,"honor_retry_after":true,"interval_s":1.0,"max_error_rate_pct":0.0,"defaults":["target","concurrency","conn_mode","requests","duration_s","rate","burst","payload_size","timeout_s","reconnect_delay_s","max_backoff_s","interval_s","max_error_rate_pct"]}"#,
            ),
        ];
        for (args, expected) in cases {
            let h = header_for(args).await;
            assert_eq!(crate::report::header_json(&h), expected, "{args:?}");
        }
    }

    #[tokio::test]
    async fn every_field_is_a_header_key() {
        let v = serde_json::to_value(header_for(&[]).await).unwrap();
        let mut keys: Vec<_> = v
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| *k != "type" && *k != "defaults")
            .cloned()
            .collect();
        let mut fields = names(&HeaderField::ALL);
        keys.sort();
        fields.sort();
        assert_eq!(keys, fields);
    }

    #[test]
    fn summary_json_line() {
        let start = Instant::now();
        let stats = Aggregator::new(start).finish(start + Duration::from_secs(2));
        let config = RunConfig {
            requests: Some(10),
            concurrency: 3,
            ..RunConfig::new(Transport::Tcp("127.0.0.1:8080".parse().unwrap()))
        };
        let s = Summary::new(RunInfo::new(&config), stats, StopReason::Interrupt);
        assert!(s.interrupted);
        assert_eq!(
            crate::report::summary_json(&s),
            r#"{"type":"summary","protocol":"tcp","target":"127.0.0.1:8080","concurrency":3,"conn_mode":"persistent","requests":10,"elapsed_s":2.0,"total":0,"ok":0,"errors":0,"errors_by_kind":{"connect_failed":0,"connect_refused":0,"mismatch":0,"other":0,"ports_exhausted":0,"rate_limited":0,"reset":0,"timeout":0},"error_rate_pct":0.0,"req_per_sec":0.0,"ok_per_sec":0.0,"latency":null,"outages":{"count":0,"total_ms":0.0,"longest_ms":0.0,"ongoing":false,"windows_truncated":false,"windows":[]},"interrupted":true,"stop_reason":"interrupt"}"#
        );
    }
}
