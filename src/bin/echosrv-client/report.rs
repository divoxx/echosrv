//! Output formatting: human-readable lines and JSON lines.
//!
//! Human renderers take a [`Palette`]; with [`Palette::PLAIN`] they produce
//! the same text as with color, minus the ANSI codes. JSON is never colored.

use crate::cli::DEFAULT_BURST;
use crate::header::{HeaderField, RunHeader};
use crate::output::{Palette, Tag, tagged};
use crate::stats::{ErrorKind, IntervalReport, OutageEvent, StopReason, Summary};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::Duration;

/// Human-readable outage windows listed in the summary.
const MAX_HUMAN_OUTAGES: usize = 10;

fn fmt_ms(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.2}ms")
    } else if ms < 1000.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{:.2}s", ms / 1000.0)
    }
}

fn dur_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn fmt_opt(d: Option<Duration>) -> String {
    d.map_or_else(|| "-".to_string(), |d| fmt_ms(dur_ms(d)))
}

/// `[    12.0s]` timestamp prefix shared by interval and outage lines.
fn timestamp(p: Palette, elapsed_s: f64) -> String {
    p.dim(&format!("[{elapsed_s:>7.1}s]"))
}

/// Errors in red when there are any, dimmed otherwise.
fn err_style(p: Palette, count: u64, s: &str) -> String {
    if count > 0 { p.red(s) } else { p.dim(s) }
}

/// `[  12.0s] 4821 req/s ok=4821 err=3 (reset=3) p50=0.18ms p99=0.92ms OUTAGE`
///
/// `OUTAGE` means an outage is still open at the end of the interval; the
/// counts cover everything that happened during it.
pub fn interval_line(r: &IntervalReport, p: Palette) -> String {
    let w = &r.window;
    let errs = w.error_count();
    let mut line = format!(
        "{} {} {} {}",
        timestamp(p, r.elapsed_s),
        p.bold(&format!("{:.0} req/s", r.rate())),
        p.green(&format!("ok={}", w.ok)),
        err_style(p, errs, &format!("err={errs}")),
    );
    let errors = w.nonzero_errors();
    if !errors.is_empty() {
        let parts: Vec<_> = errors.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = write!(line, " {}", p.red(&format!("({})", parts.join(" "))));
    }
    let _ = write!(
        line,
        " {} {}",
        p.cyan(&format!("p50={}", fmt_opt(w.percentile(0.50)))),
        p.cyan(&format!("p99={}", fmt_opt(w.percentile(0.99))))
    );
    if r.outage_open {
        let _ = write!(line, " {}", p.bold_red("OUTAGE"));
    }
    line
}

/// JSON line for one interval (fields in a fixed, readable order).
#[derive(Serialize)]
struct IntervalJson<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    elapsed_s: f64,
    interval_s: f64,
    req_per_sec: f64,
    total: u64,
    ok: u64,
    errors: u64,
    errors_by_kind: BTreeMap<&'a str, u64>,
    p50_ms: Option<f64>,
    p99_ms: Option<f64>,
    /// An outage is still open at the end of the interval.
    outage_open: bool,
}

pub fn interval_json(r: &IntervalReport) -> String {
    let w = &r.window;
    let ms = |q| w.percentile(q).map(dur_ms);
    to_json(&IntervalJson {
        kind: "interval",
        elapsed_s: r.elapsed_s,
        interval_s: r.length.as_secs_f64(),
        req_per_sec: r.rate(),
        total: w.count,
        ok: w.ok,
        errors: w.error_count(),
        errors_by_kind: w.nonzero_errors(),
        p50_ms: ms(0.50),
        p99_ms: ms(0.99),
        outage_open: r.outage_open,
    })
}

/// `[    3.2s] [fail] outage started (connect_refused)` /
/// `[    4.1s]   [ok] outage ended after 920.0ms (37 errors)`
pub fn outage_line(e: &OutageEvent, p: Palette) -> String {
    match e {
        OutageEvent::Started { at_s, kind } => format!(
            "{} {}",
            timestamp(p, *at_s),
            tagged(
                p,
                Tag::Fail,
                &format!("outage started ({})", p.red(kind.as_str()))
            )
        ),
        OutageEvent::Ended { at_s, window } => format!(
            "{} {}",
            timestamp(p, *at_s),
            tagged(
                p,
                Tag::Ok,
                &format!(
                    "outage ended after {} ({} errors)",
                    p.bold(&fmt_ms(window.duration_ms)),
                    window.errors
                )
            )
        ),
    }
}

/// JSON line for an outage start/end.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutageJson {
    OutageStart {
        elapsed_s: f64,
        kind: ErrorKind,
    },
    OutageEnd {
        elapsed_s: f64,
        start_s: f64,
        duration_ms: f64,
        errors: u64,
    },
}

pub fn outage_json(e: &OutageEvent) -> String {
    to_json(&match *e {
        OutageEvent::Started { at_s, kind } => OutageJson::OutageStart {
            elapsed_s: at_s,
            kind,
        },
        OutageEvent::Ended { at_s, window } => OutageJson::OutageEnd {
            elapsed_s: at_s,
            start_s: window.start_s,
            duration_ms: window.duration_ms,
            errors: window.errors,
        },
    })
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("report types serialize")
}

pub fn summary_json(s: &Summary) -> String {
    to_json(s)
}

/// `5s`, `100ms`, `1.5s`, `2m`, `1h`, `250us`: the largest unit that gives a
/// whole number (`90s`, `90m`), so the text parses back with `--duration`.
fn fmt_dur(d: Duration) -> String {
    let whole_secs = d.subsec_nanos() == 0;
    if d.is_zero() {
        "0s".into()
    } else if whole_secs && d.as_secs() % 3600 == 0 {
        format!("{}h", d.as_secs() / 3600)
    } else if whole_secs && d.as_secs() % 60 == 0 {
        format!("{}m", d.as_secs() / 60)
    } else if whole_secs {
        format!("{}s", d.as_secs())
    } else if d >= Duration::from_secs(1) {
        format!("{}s", d.as_secs_f64())
    } else if d.subsec_nanos() % 1_000_000 == 0 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{}us", d.as_nanos() as f64 / 1000.0)
    }
}

/// Multi-line human-readable configuration header; values that came from
/// defaults are followed by a dimmed `(default)`.
pub fn header_text(h: &RunHeader, p: Palette) -> String {
    let mark = |field: HeaderField, value: String| {
        if h.is_default(field) {
            format!("{value} {}", p.dim("(default)"))
        } else {
            value
        }
    };
    let mut out = String::new();
    let _ = writeln!(out, "{}", p.dim("--- echosrv-client ---"));
    let _ = writeln!(
        out,
        "{}{} {}",
        label(p, "target"),
        h.info.protocol,
        mark(HeaderField::Target, p.bold(&h.info.target))
    );
    let _ = writeln!(
        out,
        "{}{}, {}",
        label(p, "workers"),
        mark(HeaderField::Concurrency, h.info.concurrency.to_string()),
        mark(
            HeaderField::ConnMode,
            format!("{} connections", h.info.conn_mode)
        )
    );
    let requests = match (h.info.requests, h.duration_s) {
        (None, None) => mark(HeaderField::Requests, "unlimited, until Ctrl-C".into()),
        (Some(n), None) => n.to_string(),
        (None, Some(d)) => format!("unlimited, for {}", fmt_dur(d)),
        (Some(n), Some(d)) => format!("{n} or {}, whichever comes first", fmt_dur(d)),
    };
    let _ = writeln!(out, "{}{requests}", label(p, "requests"));
    let rate = match (h.rate, h.burst) {
        (Some(rate), burst) => format!(
            "{rate} req/s, {}",
            mark(
                HeaderField::Burst,
                format!("burst {}", burst.unwrap_or(DEFAULT_BURST))
            )
        ),
        (None, _) => mark(HeaderField::Rate, "unshaped".into()),
    };
    let conns = mark(
        HeaderField::ConnRate,
        h.conn_rate.map_or_else(
            || "unlimited new connections".into(),
            |n| format!("at most {n} new connections/s"),
        ),
    );
    let _ = writeln!(out, "{}{rate}, {conns}", label(p, "rate"));
    let size = h
        .payload_size
        .map_or_else(|| "header + text".into(), |n| format!("{n} bytes"));
    let _ = writeln!(
        out,
        "{}{}, {}",
        label(p, "payload"),
        mark(HeaderField::PayloadSize, size),
        mark(HeaderField::Filler, format!("{} filler", h.filler))
    );
    let _ = writeln!(
        out,
        "{}{}, {} {}{}",
        label(p, "timeout"),
        mark(HeaderField::Timeout, fmt_dur(h.timeout_s)),
        mark(
            HeaderField::ReconnectDelay,
            format!("backoff {}", fmt_dur(h.reconnect_delay_s))
        ),
        mark(
            HeaderField::MaxBackoff,
            format!("up to {}", fmt_dur(h.max_backoff_s))
        ),
        if h.honor_retry_after {
            ", honors Retry-After"
        } else {
            ""
        }
    );
    let interval = h.interval_s.map_or_else(|| "off".into(), fmt_dur);
    let _ = writeln!(
        out,
        "{}{}",
        label(p, "interval"),
        mark(HeaderField::Interval, interval)
    );
    let _ = writeln!(
        out,
        "{}{}",
        label(p, "max errors"),
        mark(
            HeaderField::MaxErrorRate,
            format!("{}%", h.max_error_rate_pct)
        )
    );
    out
}

pub fn header_json(h: &RunHeader) -> String {
    to_json(h)
}

/// Hint on stderr when the run stopped because the machine ran out of
/// local ports (see [`crate::runner::SAFE_CONN_RATE`]).
pub fn ports_exhausted_hint() -> String {
    format!(
        "stopped: this machine ran out of local ports (EADDRNOTAVAIL). Closed connections \
         hold their port for 30-60s, so new connections must stay under about {}/s: \
         lower --conn-rate or --rate, or use --conn-mode persistent",
        crate::runner::SAFE_CONN_RATE
    )
}

/// Pass/fail decision for the run; drives the exit code and the summary's
/// last line.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Pass {
        error_rate_pct: f64,
        max: f64,
    },
    /// The run stopped because the client machine ran out of local ports.
    PortsExhausted,
    /// The run ended before any request completed (for example Ctrl-C right
    /// after start): a load test that sent nothing does not pass.
    NoAttempts,
    Mismatches(u64),
    ErrorRate {
        error_rate_pct: f64,
        max: f64,
    },
}

impl Verdict {
    pub fn of(s: &Summary, max_error_rate: f64) -> Self {
        match s.stop_reason {
            StopReason::PortsExhausted => return Verdict::PortsExhausted,
            StopReason::Completed
            | StopReason::Duration
            | StopReason::Interrupt
            | StopReason::Terminated => {}
        }
        if s.stats.total == 0 {
            Verdict::NoAttempts
        } else if s.stats.mismatches() > 0 {
            Verdict::Mismatches(s.stats.mismatches())
        } else if s.stats.error_rate_pct > max_error_rate {
            Verdict::ErrorRate {
                error_rate_pct: s.stats.error_rate_pct,
                max: max_error_rate,
            }
        } else {
            Verdict::Pass {
                error_rate_pct: s.stats.error_rate_pct,
                max: max_error_rate,
            }
        }
    }

    pub fn passed(&self) -> bool {
        matches!(self, Verdict::Pass { .. })
    }

    /// `  [ok] pass ...` or `[fail] <reason>`.
    pub fn line(&self, p: Palette) -> String {
        match *self {
            Verdict::Pass {
                error_rate_pct,
                max,
            } => tagged(
                p,
                Tag::Ok,
                &p.green(&format!(
                    "no mismatches, error rate {error_rate_pct:.2}% within --max-error-rate {max}%"
                )),
            ),
            Verdict::PortsExhausted => tagged(
                p,
                Tag::Fail,
                &p.red("stopped: the client machine ran out of local ports"),
            ),
            Verdict::NoAttempts => tagged(p, Tag::Fail, &p.red("no requests were attempted")),
            Verdict::Mismatches(n) => tagged(p, Tag::Fail, &p.red(&format!("{n} echo mismatches"))),
            Verdict::ErrorRate {
                error_rate_pct,
                max,
            } => tagged(
                p,
                Tag::Fail,
                &p.red(&format!(
                    "error rate {error_rate_pct:.2}% above --max-error-rate {max}%"
                )),
            ),
        }
    }
}

/// A summary label padded to the value column (padding before styling, so
/// ANSI codes don't break alignment).
fn label(p: Palette, name: &str) -> String {
    p.cyan(&format!("{name:<12}"))
}

/// Multi-line human-readable summary, ending with the verdict line.
pub fn summary_text(s: &Summary, verdict: &Verdict, p: Palette) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", p.dim("--- echosrv-client summary ---"));
    let _ = writeln!(
        out,
        "{}{} {} ({}, c={})",
        label(p, "target"),
        s.info.protocol,
        p.bold(&s.info.target),
        s.info.conn_mode,
        s.info.concurrency
    );
    let reason = if s.interrupted {
        format!("{}, interrupted before -n completed", s.stop_reason)
    } else {
        s.stop_reason.to_string()
    };
    let _ = writeln!(
        out,
        "{}{:.2}s ({reason})",
        label(p, "elapsed"),
        s.stats.elapsed_s
    );
    let requested = s
        .info
        .requests
        .map_or_else(String::new, |n| format!(" of {n}"));
    let _ = writeln!(
        out,
        "{}{}{requested} total, {} ok, {}",
        label(p, "requests"),
        s.stats.total,
        p.green(&s.stats.ok.to_string()),
        err_style(
            p,
            s.stats.errors,
            &format!("{} errors ({:.2}%)", s.stats.errors, s.stats.error_rate_pct)
        )
    );
    let _ = writeln!(
        out,
        "{}{} ({:.1} ok/s)",
        label(p, "throughput"),
        p.bold(&format!("{:.1} req/s", s.stats.req_per_sec)),
        s.stats.ok_per_sec
    );
    if s.stats.errors > 0 {
        let parts: Vec<_> = s
            .stats
            .errors_by_kind
            .iter()
            .filter(|(_, v)| **v > 0)
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(out, "{}{}", label(p, "errors"), p.red(&parts.join(" ")));
    }
    match &s.stats.latency {
        Some(l) => {
            let _ = writeln!(
                out,
                "{}min={} p50={} p90={} p99={} p99.9={} max={} mean={}",
                label(p, "latency"),
                fmt_ms(l.min_ms),
                p.bold(&fmt_ms(l.p50_ms)),
                fmt_ms(l.p90_ms),
                p.bold(&fmt_ms(l.p99_ms)),
                fmt_ms(l.p999_ms),
                fmt_ms(l.max_ms),
                fmt_ms(l.mean_ms)
            );
        }
        None => {
            let _ = writeln!(
                out,
                "{}{}",
                label(p, "latency"),
                p.dim("- (no successful requests)")
            );
        }
    }
    let o = &s.stats.outages;
    if o.count == 0 {
        let _ = writeln!(out, "{}{}", label(p, "outages"), p.green("none"));
    } else {
        let _ = writeln!(
            out,
            "{}{}",
            label(p, "outages"),
            p.red(&format!(
                "{} (total {}, longest {}){}",
                o.count,
                fmt_ms(o.total_ms),
                fmt_ms(o.longest_ms),
                if o.ongoing { ", last one ongoing" } else { "" }
            ))
        );
        for w in o.windows.iter().take(MAX_HUMAN_OUTAGES) {
            let _ = writeln!(
                out,
                "            at {:>7.2}s for {} ({} errors{})",
                w.start_s,
                fmt_ms(w.duration_ms),
                w.errors,
                if w.ongoing { ", ongoing" } else { "" }
            );
        }
        if o.count > MAX_HUMAN_OUTAGES as u64 {
            let _ = writeln!(
                out,
                "            ... and {} more",
                o.count - MAX_HUMAN_OUTAGES as u64
            );
        }
    }
    let _ = writeln!(out, "{}", verdict.line(p));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::RunInfo;
    use crate::output::enable_ansi;
    use crate::stats::{Aggregator, ErrorKind, OutageWindow, Outcome, Sample, Window};
    use tokio::time::Instant;

    fn window() -> Window {
        let mut w = Window::default();
        for _ in 0..10 {
            w.record(&Sample {
                at: Instant::now(),
                latency: Duration::from_micros(180),
                outcome: Outcome::Ok,
            });
        }
        for _ in 0..3 {
            w.record(&Sample {
                at: Instant::now(),
                latency: Duration::ZERO,
                outcome: Outcome::Err(ErrorKind::Reset),
            });
        }
        w
    }

    #[test]
    fn interval_line_format() {
        let r = IntervalReport {
            elapsed_s: 12.0,
            length: Duration::from_millis(500),
            window: window(),
            outage_open: true,
        };
        assert_eq!(
            interval_line(&r, Palette::PLAIN),
            "[   12.0s] 26 req/s ok=10 err=3 (reset=3) p50=0.18ms p99=0.18ms OUTAGE"
        );
        let r = IntervalReport {
            elapsed_s: 1.0,
            length: Duration::from_secs(1),
            window: Window::default(),
            outage_open: false,
        };
        assert_eq!(
            interval_line(&r, Palette::PLAIN),
            "[    1.0s] 0 req/s ok=0 err=0 p50=- p99=-"
        );
        enable_ansi();
        let colored = interval_line(&r, Palette::new(true));
        assert!(colored.contains("\x1b["), "{colored:?}");
        assert_eq!(strip_ansi(&colored), interval_line(&r, Palette::PLAIN));

        let line = interval_json(&IntervalReport {
            elapsed_s: 2.0,
            length: Duration::from_secs(1),
            window: window(),
            outage_open: false,
        });
        assert!(line.starts_with(r#"{"type":"interval","#), "{line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "interval");
        assert_eq!(v["errors_by_kind"]["reset"], 3);
        assert!(v["errors_by_kind"].get("timeout").is_none());
        assert_eq!(v["outage_open"], false);
        assert!(v.get("outage").is_none());
    }

    fn info() -> RunInfo {
        RunInfo {
            protocol: "tcp",
            target: "127.0.0.1:8080".into(),
            concurrency: 1,
            conn_mode: "persistent",
            requests: None,
        }
    }

    fn header(defaults: &[HeaderField]) -> RunHeader {
        RunHeader {
            kind: "config",
            info: info(),
            duration_s: None,
            rate: None,
            burst: None,
            conn_rate: Some(100),
            payload_size: Some(64),
            filler: "pattern",
            timeout_s: Duration::from_secs(5),
            reconnect_delay_s: Duration::from_millis(100),
            max_backoff_s: Duration::from_millis(200),
            honor_retry_after: false,
            interval_s: Some(Duration::from_secs(1)),
            max_error_rate_pct: 0.0,
            defaults: defaults.to_vec(),
        }
    }

    #[test]
    fn header_all_defaults() {
        let h = header(&HeaderField::ALL);
        let text = header_text(&h, Palette::PLAIN);
        assert_eq!(
            text,
            "--- echosrv-client ---\n\
             target      tcp 127.0.0.1:8080 (default)\n\
             workers     1 (default), persistent connections (default)\n\
             requests    unlimited, until Ctrl-C (default)\n\
             rate        unshaped (default), at most 100 new connections/s (default)\n\
             payload     64 bytes (default), pattern filler (default)\n\
             timeout     5s (default), backoff 100ms (default) up to 200ms (default)\n\
             interval    1s (default)\n\
             max errors  0% (default)\n"
        );
        enable_ansi();
        let colored = header_text(&h, Palette::new(true));
        assert!(colored.contains("\x1b["), "{colored:?}");
        assert_eq!(strip_ansi(&colored), text);
    }

    #[test]
    fn header_explicit_values() {
        let h = RunHeader {
            info: RunInfo {
                target: "10.0.0.5:9090".into(),
                concurrency: 20,
                conn_mode: "per-request",
                requests: Some(10_000),
                ..info()
            },
            duration_s: Some(Duration::from_secs(30)),
            rate: Some(500),
            burst: Some(1),
            payload_size: None,
            filler: "text",
            timeout_s: Duration::from_millis(250),
            conn_rate: None,
            reconnect_delay_s: Duration::from_millis(1500),
            max_backoff_s: Duration::from_secs(4),
            honor_retry_after: true,
            interval_s: None,
            max_error_rate_pct: 2.5,
            ..header(&[HeaderField::Burst])
        };
        let text = header_text(&h, Palette::PLAIN);
        assert_eq!(
            text,
            "--- echosrv-client ---\n\
             target      tcp 10.0.0.5:9090\n\
             workers     20, per-request connections\n\
             requests    10000 or 30s, whichever comes first\n\
             rate        500 req/s, burst 1 (default), unlimited new connections\n\
             payload     header + text, text filler\n\
             timeout     250ms, backoff 1.5s up to 4s, honors Retry-After\n\
             interval    off\n\
             max errors  2.5%\n"
        );
        assert!(!text.contains('\x1b'));
        enable_ansi();
        assert_eq!(strip_ansi(&header_text(&h, Palette::new(true))), text);

        let only_d = RunHeader {
            duration_s: Some(Duration::from_secs(120)),
            ..header(&[HeaderField::Requests])
        };
        assert!(header_text(&only_d, Palette::PLAIN).contains("requests    unlimited, for 2m\n"));
    }

    #[test]
    fn header_json_shape() {
        let line = header_json(&header(&[HeaderField::Target, HeaderField::Rate]));
        assert!(line.starts_with(r#"{"type":"config","#), "{line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["protocol"], "tcp");
        assert_eq!(v["target"], "127.0.0.1:8080");
        assert_eq!(v["concurrency"], 1);
        assert_eq!(v["conn_mode"], "persistent");
        assert!(v["requests"].is_null() && v["rate"].is_null());
        assert_eq!(v["payload_size"], 64);
        assert_eq!(v["filler"], "pattern");
        assert_eq!(v["timeout_s"], 5.0);
        assert_eq!(v["interval_s"], 1.0);
        assert!(v["duration_s"].is_null());
        // Durations serialize as seconds, exactly as `as_secs_f64` gives them.
        assert_eq!(v["reconnect_delay_s"], 0.1);
        assert_eq!(v["max_backoff_s"], 0.2);
        assert_eq!(v["defaults"], serde_json::json!(["target", "rate"]));
        let h = RunHeader {
            duration_s: Some(Duration::from_millis(1500)),
            timeout_s: Duration::from_nanos(1),
            interval_s: None,
            ..header(&[])
        };
        let v: serde_json::Value = serde_json::from_str(&header_json(&h)).unwrap();
        assert_eq!(v["duration_s"], 1.5);
        assert_eq!(v["timeout_s"], 1e-9);
        assert!(v["interval_s"].is_null());
        // Every name in `defaults` is a field of the line.
        let all = header_json(&header(&HeaderField::ALL));
        let v: serde_json::Value = serde_json::from_str(&all).unwrap();
        for name in v["defaults"].as_array().unwrap() {
            assert!(v.get(name.as_str().unwrap()).is_some(), "{name}");
        }
    }

    #[test]
    fn durations_format() {
        let f = fmt_dur;
        assert_eq!(f(Duration::ZERO), "0s");
        assert_eq!(f(Duration::from_secs(5)), "5s");
        assert_eq!(f(Duration::from_secs(120)), "2m");
        assert_eq!(f(Duration::from_secs(90)), "90s");
        assert_eq!(f(Duration::from_secs(3600)), "1h");
        assert_eq!(f(Duration::from_secs(7200)), "2h");
        assert_eq!(f(Duration::from_secs(5400)), "90m");
        assert_eq!(f(Duration::from_secs(3601)), "3601s");
        assert_eq!(f(Duration::from_secs(60)), "1m");
        assert_eq!(f(Duration::from_secs(1)), "1s");
        assert_eq!(f(Duration::from_millis(3_600_500)), "3600.5s");
        assert_eq!(f(Duration::from_millis(1500)), "1.5s");
        assert_eq!(f(Duration::from_millis(100)), "100ms");
        assert_eq!(f(Duration::from_micros(250)), "250us");
        // Every output parses back to the same duration.
        for d in [
            Duration::from_secs(3600),
            Duration::from_secs(5400),
            Duration::from_secs(90),
            Duration::from_millis(1500),
            Duration::from_millis(100),
            Duration::from_micros(250),
        ] {
            assert_eq!(crate::cli::parse_duration(&f(d)), Ok(d), "{}", f(d));
        }
    }

    #[test]
    fn outage_lines() {
        let started = OutageEvent::Started {
            at_s: 3.2,
            kind: ErrorKind::ConnectRefused,
        };
        assert_eq!(
            outage_line(&started, Palette::PLAIN),
            "[    3.2s] [fail] outage started (connect_refused)"
        );
        let line = outage_json(&started);
        assert!(line.starts_with(r#"{"type":"outage_start","#), "{line}");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["kind"], "connect_refused");
        let ended = OutageEvent::Ended {
            at_s: 4.1,
            window: OutageWindow {
                start_s: 3.2,
                duration_ms: 920.0,
                errors: 37,
                ongoing: false,
            },
        };
        assert_eq!(
            outage_line(&ended, Palette::PLAIN),
            "[    4.1s]   [ok] outage ended after 920.0ms (37 errors)"
        );
        enable_ansi();
        let colored = outage_line(&ended, Palette::new(true));
        assert!(colored.contains("\x1b["));
        assert_eq!(strip_ansi(&colored), outage_line(&ended, Palette::PLAIN));
    }

    /// Removes `ESC [ ... m` sequences.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn verdicts() {
        let start = Instant::now();
        let stats = Aggregator::new(start).finish(start + Duration::from_secs(1));
        let mut s = Summary::new(info(), stats, StopReason::Completed);
        s.stats.total = 100;
        s.stats.error_rate_pct = 5.0;
        let v = Verdict::of(&s, 1.0);
        assert!(!v.passed());
        assert_eq!(
            v.line(Palette::PLAIN),
            "[fail] error rate 5.00% above --max-error-rate 1%"
        );
        let v = Verdict::of(&s, 5.0);
        assert!(v.passed());
        assert_eq!(
            v.line(Palette::PLAIN),
            "  [ok] no mismatches, error rate 5.00% within --max-error-rate 5%"
        );
        s.stats
            .errors_by_kind
            .insert(ErrorKind::Mismatch.as_str(), 2);
        assert_eq!(Verdict::of(&s, 100.0), Verdict::Mismatches(2));
        assert_eq!(
            Verdict::Mismatches(2).line(Palette::PLAIN),
            "[fail] 2 echo mismatches"
        );

        // Running out of local ports fails the run whatever the error rate.
        s.stop_reason = StopReason::PortsExhausted;
        let v = Verdict::of(&s, 100.0);
        assert_eq!(v, Verdict::PortsExhausted);
        assert!(!v.passed());
        assert_eq!(
            v.line(Palette::PLAIN),
            "[fail] stopped: the client machine ran out of local ports"
        );
        // Any other way of stopping leaves the verdict to the results.
        s.stats.errors_by_kind.clear();
        for reason in [
            StopReason::Completed,
            StopReason::Duration,
            StopReason::Interrupt,
            StopReason::Terminated,
        ] {
            s.stop_reason = reason;
            assert!(Verdict::of(&s, 5.0).passed(), "{reason}");
        }
    }

    #[test]
    fn no_attempts_fails() {
        let start = Instant::now();
        let stats = Aggregator::new(start).finish(start + Duration::from_secs(1));
        let s = Summary::new(info(), stats, StopReason::Completed);
        assert_eq!(s.stats.total, 0);
        let v = Verdict::of(&s, 100.0);
        assert_eq!(v, Verdict::NoAttempts);
        assert!(!v.passed());
        assert_eq!(v.line(Palette::PLAIN), "[fail] no requests were attempted");
        // Running out of ports is the more specific reason.
        let mut s = s;
        s.stop_reason = StopReason::PortsExhausted;
        assert_eq!(Verdict::of(&s, 100.0), Verdict::PortsExhausted);
    }

    #[test]
    fn summary_formats() {
        let start = Instant::now();
        let stats = Aggregator::new(start).finish(start + Duration::from_secs(2));
        let mut agg_summary = Summary::new(info(), stats, StopReason::Completed);
        let verdict = Verdict::of(&agg_summary, 0.0);
        assert_eq!(verdict, Verdict::NoAttempts);
        let text = summary_text(&agg_summary, &verdict, Palette::PLAIN);
        assert!(!text.contains('\x1b'));
        assert!(text.contains("outages     none"));
        assert!(text.contains("no successful requests"));
        assert_eq!(
            text.lines().last(),
            Some("[fail] no requests were attempted")
        );
        enable_ansi();
        let colored = summary_text(&agg_summary, &verdict, Palette::new(true));
        assert!(colored.contains("\x1b["));
        assert_eq!(strip_ansi(&colored), text);
        let v: serde_json::Value = serde_json::from_str(&summary_json(&agg_summary)).unwrap();
        assert_eq!(v["type"], "summary");
        assert_eq!(v["protocol"], "tcp");
        assert_eq!(v["interrupted"], false);
        assert_eq!(v["stop_reason"], "completed");
        assert!(text.contains("(completed)"), "{text}");

        // Stop reasons keep their snake_case names in text and JSON.
        agg_summary.stop_reason = StopReason::PortsExhausted;
        agg_summary.interrupted = true;
        let v: serde_json::Value = serde_json::from_str(&summary_json(&agg_summary)).unwrap();
        assert_eq!(v["stop_reason"], "ports_exhausted");
        let text = summary_text(&agg_summary, &Verdict::PortsExhausted, Palette::PLAIN);
        assert!(
            text.contains("(ports_exhausted, interrupted before -n completed)"),
            "{text}"
        );
    }

    #[test]
    fn ports_exhausted_hint_names_the_safe_rate() {
        let hint = ports_exhausted_hint();
        let rate = format!("about {}/s", crate::runner::SAFE_CONN_RATE);
        assert!(hint.contains(&rate), "{hint}");
        assert!(hint.contains("--conn-mode persistent"), "{hint}");
    }
}
