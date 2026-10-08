//! `echosrv-client`: load / stress-test client for echosrv servers.
//!
//! Exit codes: 0 ok, 1 mismatch, error rate above `--max-error-rate` or
//! local ports exhausted, 2 usage or setup error, 130 / 143 aborted by a
//! second SIGINT / SIGTERM, 141 stdout closed
//! (e.g. `| head`).

mod cli;
mod output;
mod report;
mod runner;
mod stats;

use cli::Cli;
use echosrv::cli::color::{ColorEnv, resolve_color};
use echosrv::cli::{help, init_logging};
use output::Palette;
use report::Verdict;
use stats::LiveEvent;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

const EXIT_FAILURE: u8 = 1;
const EXIT_USAGE: u8 = 2;
/// 128 + SIGINT / SIGTERM, for a run aborted by a second signal.
const EXIT_INTERRUPTED: u8 = 130;
const EXIT_TERMINATED: u8 = 143;
/// 128 + SIGPIPE, what a shell reports for a process killed by a closed pipe.
const EXIT_BROKEN_PIPE: u8 = 141;

/// Writes one line to stdout. If stdout was closed (e.g. `| head` has
/// exited), nobody reads the report any more: exit quietly, like a process
/// killed by SIGPIPE, instead of running on unseen.
fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let result = writeln!(out, "{line}").and_then(|()| out.flush());
    if result.is_err_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) {
        std::process::exit(i32::from(EXIT_BROKEN_PIPE));
    }
}

/// Palettes for stdout (the report) and stderr (diagnostics), each decided
/// against its own stream. JSON on stdout is never colored.
fn palettes(cli: &Cli) -> (Palette, Palette) {
    let env = ColorEnv::from_env();
    let stdout = !cli.json && resolve_color(cli.color, std::io::stdout().is_terminal(), env);
    let stderr = resolve_color(cli.color, std::io::stderr().is_terminal(), env);
    if stdout || stderr {
        output::enable_ansi();
    }
    (Palette::new(stdout), Palette::new(stderr))
}

#[tokio::main]
async fn main() -> ExitCode {
    // clap exits with 2 on usage errors and 0 for --help/--version.
    let (cli, matches) = help::parse_with_matches::<Cli>();
    let (out_palette, err_palette) = palettes(&cli);
    init_logging(
        if cli.verbose { "debug" } else { "warn" },
        err_palette != Palette::PLAIN,
    );

    let validated = match cli.resolve().await {
        Ok(v) => v,
        Err(e) => {
            output::fail(err_palette, &e);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    for w in &validated.warnings {
        output::warn(err_palette, w);
    }

    // Register the handlers before the header is printed and the run (and
    // the `--duration` timer) starts: a signal that arrived before them
    // would get the default action and kill the client without a summary.
    let signals = match (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
    ) {
        (Ok(int), Ok(term)) => (int, term),
        (Err(e), _) | (_, Err(e)) => {
            output::fail(
                err_palette,
                &format!("cannot install the SIGINT / SIGTERM handlers: {e}"),
            );
            return ExitCode::from(EXIT_USAGE);
        }
    };

    // The resolved configuration goes first, even with `-i 0`.
    let header = cli.header(&matches, &validated.config);
    if cli.json {
        emit(&report::header_json(&header));
    } else {
        // Ends with a newline, so `emit` leaves a blank line after it.
        emit(&report::header_text(&header, out_palette));
    }

    let cancel = CancellationToken::new();
    let stop_reason: Arc<OnceLock<&'static str>> = Arc::new(OnceLock::new());

    // The first SIGINT (Ctrl-C) or SIGTERM stops gracefully: no new requests
    // start, and the ones in flight finish and are reported. A second
    // signal aborts at once with 130 / 143.
    {
        let cancel = cancel.clone();
        let stop_reason = stop_reason.clone();
        let (mut int, mut term) = signals;
        tokio::spawn(async move {
            let reason = tokio::select! {
                _ = int.recv() => "interrupt",
                _ = term.recv() => "terminated",
            };
            let _ = stop_reason.set(reason);
            output::info(
                err_palette,
                "stopping: letting requests in flight finish (signal again to abort)",
            );
            cancel.cancel();
            let code = tokio::select! {
                _ = int.recv() => EXIT_INTERRUPTED,
                _ = term.recv() => EXIT_TERMINATED,
            };
            output::fail(err_palette, "aborted");
            std::process::exit(i32::from(code));
        });
    }
    if let Some(duration) = cli.duration {
        let cancel = cancel.clone();
        let stop_reason = stop_reason.clone();
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            let _ = stop_reason.set("duration");
            cancel.cancel();
        });
    }

    let json = cli.json;
    let on_event = move |event: LiveEvent| {
        let line = match (&event, json) {
            (LiveEvent::Interval(r), false) => report::interval_line(r, out_palette),
            (LiveEvent::Interval(r), true) => report::interval_json(r),
            (LiveEvent::Outage(e), false) => report::outage_line(e, out_palette),
            (LiveEvent::Outage(e), true) => report::outage_json(e),
        };
        emit(&line);
    };

    let mut summary = runner::run(validated.config, cancel.clone(), on_event).await;
    if summary.stop_reason == runner::STOP_PORTS_EXHAUSTED {
        output::fail(err_palette, &report::ports_exhausted_hint());
    } else if cancel.is_cancelled() {
        summary.stop_reason = stop_reason.get().copied().unwrap_or("completed");
    }
    let verdict = Verdict::of(&summary, cli.max_error_rate);
    if json {
        emit(&report::summary_json(&summary));
    } else {
        let text = report::summary_text(&summary, &verdict, out_palette);
        emit(text.trim_end());
    }
    if verdict.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILURE)
    }
}
