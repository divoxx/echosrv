//! `echosrv-client`: load / stress-test client for echosrv servers.
//!
//! Exit codes: 0 ok, 1 mismatch, error rate above `--max-error-rate` or
//! local ports exhausted, 2 usage or setup error, 130 / 143 aborted by a
//! second SIGINT / SIGTERM, 141 stdout closed
//! (e.g. `| head`).

mod cli;
mod header;
mod output;
mod report;
mod runner;
mod stats;
#[cfg(test)]
#[path = "../../../tests/common/mod.rs"]
mod test_common;

use clap::ArgMatches;
use cli::Cli;
use echosrv::cli::color::{ColorEnv, resolve_color};
use echosrv::cli::{help, init_logging};
use header::RunHeader;
use output::Palette;
use report::Verdict;
use runner::RunConfig;
use stats::{LiveEvent, StopReason, Summary};
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

/// Prints the resolved configuration, before anything else (even with
/// `-i 0`).
fn emit_header(cli: &Cli, config: &RunConfig, matches: &ArgMatches, p: Palette) {
    let header = RunHeader::new(config, cli.max_error_rate, header::defaulted(matches));
    if cli.json {
        emit(&report::header_json(&header));
    } else {
        // Ends with a newline, so `emit` leaves a blank line after it.
        emit(&report::header_text(&header, p));
    }
}

/// Handles SIGINT (Ctrl-C) and SIGTERM. The first signal stops gracefully:
/// no new requests start, and the ones in flight finish and are reported. A
/// second signal aborts at once with 130 / 143.
///
/// The handlers are registered before this returns, so call it before the
/// header is printed and the run (and its `--duration` timer) starts: a
/// signal that arrived before them would get the default action and kill
/// the client without a summary.
fn install_signals(
    cancel: CancellationToken,
    stop_reason: Arc<OnceLock<StopReason>>,
    err: Palette,
) -> std::io::Result<()> {
    let mut int = signal(SignalKind::interrupt())?;
    let mut term = signal(SignalKind::terminate())?;
    tokio::spawn(async move {
        let reason = tokio::select! {
            _ = int.recv() => StopReason::Interrupt,
            _ = term.recv() => StopReason::Terminated,
        };
        output::info(
            err,
            "stopping: letting requests in flight finish (signal again to abort)",
        );
        runner::stop(&cancel, &stop_reason, reason);
        let code = tokio::select! {
            _ = int.recv() => EXIT_INTERRUPTED,
            _ = term.recv() => EXIT_TERMINATED,
        };
        output::fail(err, "aborted");
        std::process::exit(i32::from(code));
    });
    Ok(())
}

/// Prints each live interval and outage event as it happens.
fn live_printer(json: bool, p: Palette) -> impl FnMut(LiveEvent) {
    move |event: LiveEvent| {
        let line = match (&event, json) {
            (LiveEvent::Interval(r), false) => report::interval_line(r, p),
            (LiveEvent::Interval(r), true) => report::interval_json(r),
            (LiveEvent::Outage(e), false) => report::outage_line(e, p),
            (LiveEvent::Outage(e), true) => report::outage_json(e),
        };
        emit(&line);
    }
}

/// Prints the summary (after a stderr explanation if the run ran out of
/// local ports) and returns the verdict.
fn emit_summary(summary: &Summary, cli: &Cli, out: Palette, err: Palette) -> Verdict {
    if summary.stop_reason == StopReason::PortsExhausted {
        output::fail(err, &report::ports_exhausted_hint());
    }
    let verdict = Verdict::of(summary, cli.max_error_rate);
    if cli.json {
        emit(&report::summary_json(summary));
    } else {
        let text = report::summary_text(summary, &verdict, out);
        emit(text.trim_end());
    }
    verdict
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

    let cancel = CancellationToken::new();
    let stop_reason: Arc<OnceLock<StopReason>> = Arc::new(OnceLock::new());
    if let Err(e) = install_signals(cancel.clone(), stop_reason.clone(), err_palette) {
        output::fail(
            err_palette,
            &format!("cannot install the SIGINT / SIGTERM handlers: {e}"),
        );
        return ExitCode::from(EXIT_USAGE);
    }
    emit_header(&cli, &validated.config, &matches, out_palette);

    let on_event = live_printer(cli.json, out_palette);
    let summary = runner::run(validated.config, cancel, stop_reason, on_event).await;
    if emit_summary(&summary, &cli, out_palette, err_palette).passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILURE)
    }
}
