//! Log output of both binaries: `tracing` events on stderr.

use tracing_subscriber::EnvFilter;

/// Installs the global `tracing` subscriber: events go to stderr, colored
/// when `ansi` is set (see [`resolve_color`](super::color::resolve_color)).
///
/// `RUST_LOG`, when set to a valid filter, overrides `default_filter`. Each
/// binary picks its own default: the server shows its own (and the
/// library's) events at `--log-level`, as `echosrv=<level>`, and keeps other
/// crates quiet; the client shows warnings from every crate, or everything
/// at debug level with `--verbose`.
pub fn init_logging(default_filter: &str, ansi: bool) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(ansi)
        .init();
}
