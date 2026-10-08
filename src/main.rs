//! The `echosrv` command-line echo server. Run `echosrv --help` for usage.

use clap::error::ErrorKind;
use clap::parser::ValueSource;
use clap::{ArgMatches, Parser, ValueEnum};
use color_eyre::eyre::{Result, WrapErr};
use echosrv::cli::color::{ColorChoice, ColorEnv, resolve_color};
use echosrv::cli::target::parse_host;
use echosrv::cli::{Protocol, Target, help, init_logging};
use echosrv::defaults::{DEFAULT_HOST, DEFAULT_PORT, DEFAULT_PROTOCOL};
use echosrv::http::{DEFAULT_MAX_BODY_SIZE, HttpConfig, HttpEchoServer};
use echosrv::network::FdInheritanceConfig;
use echosrv::tcp::TcpConfig;
use echosrv::udp::UdpConfig;
use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
use echosrv::{
    EchoServerTrait, RateLimitConfig, TcpEchoServer, UdpEchoServer, UnixDatagramEchoServer,
    UnixStreamEchoServer,
};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tracing::info;

/// Connection limit of the tcp and http servers started by the CLI. The Unix
/// stream server keeps the library default
/// ([`UnixStreamConfig::default`]: 100).
const DEFAULT_MAX_CONNECTIONS: usize = 1000;

/// Log levels for `--log-level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

const AFTER_HELP: &str = "\
Examples:
  echosrv                              TCP on 127.0.0.1:8080
  echosrv --host 0.0.0.0 udp 9090      UDP on all interfaces, port 9090
  echosrv http --rate 100 --burst 10   HTTP limited to 100 requests/s
  echosrv unix-stream /tmp/echo.sock   Unix stream socket at a custom path";

const AFTER_LONG_HELP: &str = "\
Environment:
  RUST_LOG       Log filter; overrides --log-level when set (e.g. echosrv=debug)
  NO_COLOR       Disables colored logs (they are only colored on a terminal)
  CLICOLOR_FORCE Colors logs even when stderr is not a terminal
  LISTEN_PID, LISTEN_FDS, LISTEN_FDNAMES
                 systemd socket activation. When present, the server uses the
                 inherited socket named after the protocol (e.g. \"tcp\"), or the
                 only socket passed, instead of binding.

Rate limits reject, they never delay: tcp resets the connection, http answers
429 Too Many Requests with Retry-After, unix-stream closes the connection, and
udp/unix-dgram drop the datagram.

The server stops gracefully on SIGINT (Ctrl-C) or SIGTERM. Logs go to stderr.

Examples:
  echosrv                              TCP on 127.0.0.1:8080
  echosrv --host 0.0.0.0 udp 9090      UDP on all interfaces, port 9090
  echosrv http --rate 100 --burst 10   HTTP limited to 100 requests/s
  echosrv unix-stream /tmp/echo.sock   Unix stream socket at a custom path";

/// Async echo server for TCP, UDP, HTTP and Unix domain sockets.
#[derive(Debug, Parser)]
#[command(
    name = "echosrv",
    version,
    after_help = AFTER_HELP,
    after_long_help = AFTER_LONG_HELP
)]
struct Args {
    /// Protocol to serve
    #[arg(value_name = "PROTOCOL", default_value = DEFAULT_PROTOCOL, value_parser = Protocol::parser())]
    protocol: Protocol,

    /// Port for tcp/udp/http, or socket path for unix-stream/unix-dgram
    /// [default: 8080 for tcp/udp/http, /tmp/echosrv_stream.sock for
    /// unix-stream, /tmp/echosrv_datagram.sock for unix-dgram]
    #[arg(value_name = "PORT|SOCKET_PATH")]
    target: Option<OsString>,

    /// IP address (IPv4 or IPv6) to bind for tcp/udp/http
    #[arg(long, value_name = "ADDR", default_value_t = DEFAULT_HOST, value_parser = parse_host)]
    host: IpAddr,

    /// Request rate limit in requests/s (datagrams/s for udp/unix-dgram);
    /// excess is rejected [default: unlimited]
    #[arg(long, value_name = "PER_SEC", value_parser = clap::value_parser!(u32).range(1..))]
    rate: Option<u32>,

    /// Request burst size [default: same as --rate]
    #[arg(long, value_name = "N", requires = "rate", value_parser = clap::value_parser!(u32).range(1..))]
    burst: Option<u32>,

    /// New-connection rate limit in connections/s (tcp, http, unix-stream);
    /// excess is rejected [default: unlimited]
    #[arg(long, value_name = "PER_SEC", value_parser = clap::value_parser!(u32).range(1..))]
    accept_rate: Option<u32>,

    /// New-connection burst size [default: same as --accept-rate]
    #[arg(long, value_name = "N", requires = "accept_rate", value_parser = clap::value_parser!(u32).range(1..))]
    accept_burst: Option<u32>,

    /// Maximum concurrent connections (tcp, http, unix-stream)
    /// [default: 1000 for tcp/http, 100 for unix-stream]
    #[arg(long, value_name = "N")]
    max_connections: Option<NonZeroUsize>,

    /// Log level (RUST_LOG overrides it)
    #[arg(long, value_name = "LEVEL", value_enum, default_value_t = LogLevel::Info)]
    log_level: LogLevel,
}

/// The validated command line.
#[derive(Debug)]
struct Cli {
    protocol: Protocol,
    host: IpAddr,
    port: u16,
    /// The socket path for unix-stream/unix-dgram (the default one when
    /// omitted); `None` for tcp/udp/http.
    socket_path: Option<PathBuf>,
    max_connections: Option<usize>,
    rate_limit: Option<RateLimitConfig>,
    accept_rate_limit: Option<RateLimitConfig>,
    log_level: LogLevel,
}

impl Cli {
    /// Checks the combinations clap cannot express and resolves the
    /// positional target. `matches` tells explicit options from defaults.
    fn resolve(args: Args, matches: &ArgMatches) -> std::result::Result<Self, (ErrorKind, String)> {
        let protocol = args.protocol;
        let explicit = |id: &str| matches.value_source(id) == Some(ValueSource::CommandLine);

        if protocol.is_unix() && explicit("host") {
            return Err((
                ErrorKind::ArgumentConflict,
                "--host cannot be used with Unix domain sockets".to_string(),
            ));
        }
        if protocol.is_datagram() {
            for (id, flag) in [
                ("accept_rate", "--accept-rate"),
                ("max_connections", "--max-connections"),
            ] {
                if explicit(id) {
                    return Err((
                        ErrorKind::ArgumentConflict,
                        format!("{flag} cannot be used with {protocol} (it has no connections)"),
                    ));
                }
            }
        }

        let target = match &args.target {
            Some(value) => Target::parse_listen(protocol, value)
                .map_err(|msg| (ErrorKind::InvalidValue, msg))?,
            None => Target::default_for(protocol),
        };
        let (port, socket_path) = match target {
            Target::Net { port, .. } => (port, None),
            Target::Unix(path) => (DEFAULT_PORT, Some(path)),
        };

        Ok(Self {
            protocol,
            host: args.host,
            port,
            socket_path,
            max_connections: args.max_connections.map(NonZeroUsize::get),
            rate_limit: args
                .rate
                .map(|rate| RateLimitConfig::new(rate, args.burst.unwrap_or(rate))),
            accept_rate_limit: args
                .accept_rate
                .map(|rate| RateLimitConfig::new(rate, args.accept_burst.unwrap_or(rate))),
            log_level: args.log_level,
        })
    }
}

/// Parses the command line (`args` includes the program name).
///
/// `--help` and `--version` come back as errors whose `use_stderr()` is
/// false; printing them shows the text.
fn parse_cli<I, A>(args: I) -> std::result::Result<Cli, clap::Error>
where
    I: IntoIterator<Item = A>,
    A: Into<OsString> + Clone,
{
    let (args, matches) = help::try_parse_from::<Args, _, _>(args)?;
    Cli::resolve(args, &matches).map_err(|(kind, msg)| help::command::<Args>().error(kind, msg))
}

fn main() -> ExitCode {
    let cli = match parse_cli(std::env::args_os()) {
        Ok(cli) => cli,
        Err(e) => {
            // Help and version are reported through clap's error type too.
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("Error: {report:?}");
            ExitCode::FAILURE
        }
    }
}

/// Logs are colored when stderr is a terminal, unless `NO_COLOR` is set.
/// `CLICOLOR_FORCE` colors them even when stderr is not a terminal.
fn stderr_wants_color() -> bool {
    resolve_color(
        ColorChoice::Auto,
        std::io::stderr().is_terminal(),
        ColorEnv::from_env(),
    )
}

fn run(cli: Cli) -> Result<()> {
    color_eyre::install()?;
    init_logging(
        &format!("echosrv={}", cli.log_level.as_str()),
        stderr_wants_color(),
    );

    // Take ownership of socket-activation descriptors (if any) and clear the
    // variables while the process is still single-threaded, so they are not
    // passed on to child processes. The servers below use the same
    // process-wide pool: the socket named after the protocol, else the only
    // socket passed, else they bind.
    FdInheritanceConfig::from_systemd_env()?;
    if std::env::var_os("LISTEN_FDS").is_some() {
        for var in ["LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES"] {
            // SAFETY: no other threads exist yet (the Tokio runtime is built
            // below), so nothing can read the environment concurrently.
            unsafe { std::env::remove_var(var) };
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .wrap_err("Failed to start Tokio runtime")?;
    runtime.block_on(start(cli))
}

async fn start(cli: Cli) -> Result<()> {
    let bind_addr = SocketAddr::new(cli.host, cli.port);
    let service_name = cli.protocol.service_name().to_string();
    let rate_limit = cli.rate_limit;
    let accept_rate_limit = cli.accept_rate_limit;

    match cli.protocol {
        Protocol::Tcp => {
            let config = TcpConfig {
                bind_addr,
                max_connections: cli.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS),
                buffer_size: 1024,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                rate_limit,
                accept_rate_limit,
                ..Default::default()
            }
            .with_fd_inheritance(service_name);
            info!(address = %config.bind_addr, max_connections = config.max_connections, ?rate_limit, ?accept_rate_limit, "Starting TCP echo server");
            serve(TcpEchoServer::new(config.into()))
                .await
                .wrap_err("Failed to run TCP echo server")
        }
        Protocol::Udp => {
            let config = UdpConfig {
                bind_addr,
                rate_limit,
                ..Default::default()
            }
            .with_fd_inheritance(service_name);
            info!(address = %config.bind_addr, ?rate_limit, "Starting UDP echo server");
            serve(UdpEchoServer::new(config.into()))
                .await
                .wrap_err("Failed to run UDP echo server")
        }
        Protocol::Http => {
            let config = HttpConfig {
                bind_addr,
                max_connections: cli.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS),
                buffer_size: 8192,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                max_body_size: DEFAULT_MAX_BODY_SIZE,
                rate_limit,
                accept_rate_limit,
                ..Default::default()
            }
            .with_fd_inheritance(service_name);
            info!(address = %config.bind_addr, max_connections = config.max_connections, ?rate_limit, ?accept_rate_limit, "Starting HTTP echo server");
            serve(HttpEchoServer::new(config))
                .await
                .wrap_err("Failed to run HTTP echo server")
        }
        Protocol::UnixStream => {
            let path = cli.socket_path.unwrap_or_default();
            let defaults = UnixStreamConfig::default();
            let config = UnixStreamConfig {
                max_connections: cli.max_connections.unwrap_or(defaults.max_connections),
                rate_limit,
                accept_rate_limit,
                ..defaults
            }
            .with_fd_inheritance(service_name, path.clone());
            info!(socket_path = %path.display(), max_connections = config.max_connections, ?rate_limit, ?accept_rate_limit, "Starting Unix domain stream echo server");
            serve(UnixStreamEchoServer::new(config))
                .await
                .wrap_err("Failed to run Unix domain stream echo server")
        }
        Protocol::UnixDatagram => {
            let path = cli.socket_path.unwrap_or_default();
            let config = UnixDatagramConfig {
                rate_limit,
                ..Default::default()
            }
            .with_fd_inheritance(service_name, path.clone());
            info!(socket_path = %path.display(), ?rate_limit, "Starting Unix domain datagram echo server");
            serve(UnixDatagramEchoServer::new(config))
                .await
                .wrap_err("Failed to run Unix domain datagram echo server")
        }
    }
}

/// Runs `server` until SIGINT or SIGTERM triggers a graceful shutdown.
async fn serve<S: EchoServerTrait>(server: S) -> echosrv::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).map_err(echosrv::EchoError::Unix)?;
    let mut sigterm = signal(SignalKind::terminate()).map_err(echosrv::EchoError::Unix)?;
    let shutdown = server.shutdown_signal();

    tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv() => info!("Received SIGINT, shutting down"),
            _ = sigterm.recv() => info!("Received SIGTERM, shutting down"),
        }
        let _ = shutdown.send(());
    });

    server.run().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use echosrv::defaults::{DEFAULT_UNIX_DGRAM_PATH, DEFAULT_UNIX_STREAM_PATH};

    /// Outcome of parsing, as the tests below expect it.
    #[derive(Debug)]
    enum Command {
        Run(Cli),
        Help,
        Version,
    }

    /// [`parse_cli`] over `args` (without the program name), with help and
    /// version mapped to [`Command`] variants.
    fn parse_args(args: &[String]) -> std::result::Result<Command, clap::Error> {
        match parse_cli(std::iter::once("echosrv".to_string()).chain(args.iter().cloned())) {
            Ok(cli) => Ok(Command::Run(cli)),
            Err(e) if e.kind() == ErrorKind::DisplayHelp => Ok(Command::Help),
            Err(e) if e.kind() == ErrorKind::DisplayVersion => Ok(Command::Version),
            Err(e) => Err(e),
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn run_cli(list: &[&str]) -> Cli {
        match parse_args(&args(list)).unwrap() {
            Command::Run(cli) => cli,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn defaults_to_tcp_on_localhost_8080() {
        let cli = run_cli(&[]);
        assert_eq!(cli.protocol, Protocol::Tcp);
        assert_eq!(
            SocketAddr::new(cli.host, cli.port),
            "127.0.0.1:8080".parse().unwrap()
        );
    }

    #[test]
    fn parses_host_and_port() {
        let cli = run_cli(&["--host", "::1", "udp", "9090"]);
        assert_eq!(cli.protocol, Protocol::Udp);
        assert_eq!(cli.host, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(cli.port, 9090);
        let cli = run_cli(&["http", "--host=[::]"]);
        assert!(cli.host.is_unspecified());
    }

    #[test]
    fn accepts_both_unix_datagram_names() {
        assert_eq!(run_cli(&["unix-dgram"]).protocol, Protocol::UnixDatagram);
        assert_eq!(
            run_cli(&["unix-datagram", "/tmp/x.sock"]).protocol,
            Protocol::UnixDatagram
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_args(&args(&["tcp", "notaport"])).is_err());
        assert!(parse_args(&args(&["tcp", "70000"])).is_err());
        assert!(parse_args(&args(&["gopher"])).is_err());
        assert!(parse_args(&args(&["--host", "nope", "tcp"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
        assert!(parse_args(&args(&["--host", "::1", "unix-stream"])).is_err());
    }

    #[test]
    fn help_and_version() {
        assert!(matches!(
            parse_args(&args(&["tcp", "-h"])),
            Ok(Command::Help)
        ));
        assert!(matches!(
            parse_args(&args(&["--version"])),
            Ok(Command::Version)
        ));
    }

    #[test]
    fn cli_definition_is_consistent() {
        help::command::<Args>().debug_assert();
    }

    #[test]
    fn protocol_names_are_case_insensitive_and_unknown_ones_are_named() {
        assert_eq!(run_cli(&["HTTP"]).protocol, Protocol::Http);
        assert_eq!(run_cli(&["Unix-Stream"]).protocol, Protocol::UnixStream);
        let err = parse_args(&args(&["gopher"])).unwrap_err().to_string();
        assert!(err.contains("unknown protocol 'gopher'"), "{err}");
        assert!(err.contains("unix-dgram"), "{err}");
    }

    #[test]
    fn unix_targets_are_paths() {
        let cli = run_cli(&["unix-stream"]);
        assert_eq!(
            cli.socket_path,
            Some(PathBuf::from(DEFAULT_UNIX_STREAM_PATH))
        );
        assert_eq!(run_cli(&["tcp"]).socket_path, None);
        assert!(parse_args(&args(&["unix-dgram", ""])).is_err());
        let cli = run_cli(&["unix-dgram", "/tmp/x.sock"]);
        assert_eq!(cli.socket_path, Some(PathBuf::from("/tmp/x.sock")));
    }

    #[test]
    fn rate_flags() {
        let cli = run_cli(&[]);
        assert_eq!(cli.rate_limit, None);
        assert_eq!(cli.accept_rate_limit, None);

        let cli = run_cli(&[
            "http",
            "--rate",
            "100",
            "--accept-rate",
            "5",
            "--accept-burst",
            "2",
        ]);
        assert_eq!(cli.rate_limit, Some(RateLimitConfig::new(100, 100)));
        assert_eq!(cli.accept_rate_limit, Some(RateLimitConfig::new(5, 2)));

        let cli = run_cli(&["udp", "--rate", "7", "--burst", "3"]);
        assert_eq!(cli.rate_limit, Some(RateLimitConfig::new(7, 3)));

        for bad in [
            &["http", "--burst", "5"][..],
            &["http", "--accept-burst", "5"],
            &["http", "--rate", "0"],
            &["http", "--rate", "-1"],
            &["http", "--rate", "1", "--burst", "0"],
            &["http", "--accept-rate", "0"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn connection_flags_are_rejected_for_datagram_protocols() {
        for protocol in ["udp", "unix-dgram"] {
            for flag in [["--accept-rate", "5"], ["--max-connections", "5"]] {
                let err = parse_args(&args(&[protocol, flag[0], flag[1]]))
                    .unwrap_err()
                    .to_string();
                assert!(err.contains(flag[0]), "{err}");
            }
        }
    }

    #[test]
    fn max_connections_and_log_level() {
        let cli = run_cli(&[]);
        assert_eq!(cli.max_connections, None);
        assert_eq!(cli.log_level, LogLevel::Info);

        let cli = run_cli(&[
            "unix-stream",
            "--max-connections",
            "7",
            "--log-level",
            "warn",
        ]);
        assert_eq!(cli.max_connections, Some(7));
        assert_eq!(cli.log_level, LogLevel::Warn);

        assert!(parse_args(&args(&["--max-connections", "0"])).is_err());
        assert!(parse_args(&args(&["--log-level", "loud"])).is_err());
    }

    #[test]
    fn help_states_every_default() {
        let long = help::command::<Args>().render_long_help().to_string();
        let short = help::command::<Args>().render_help().to_string();
        let unix_max = UnixStreamConfig::default().max_connections;
        for needle in [
            format!("[default: {DEFAULT_PROTOCOL}]"),
            format!("[default: {DEFAULT_HOST}]"),
            format!("[default: {DEFAULT_PORT} for tcp/udp/http"),
            DEFAULT_UNIX_STREAM_PATH.to_string(),
            DEFAULT_UNIX_DGRAM_PATH.to_string(),
            "[default: unlimited]".to_string(),
            "[default: same as --rate]".to_string(),
            "[default: same as --accept-rate]".to_string(),
            format!(
                "[default: {DEFAULT_MAX_CONNECTIONS} for tcp/http, {unix_max} for unix-stream]"
            ),
            "[default: info]".to_string(),
        ] {
            assert!(long.contains(&needle), "--help lacks {needle:?}:\n{long}");
            assert!(short.contains(&needle), "-h lacks {needle:?}:\n{short}");
        }
        // In --help every default sits on its own line, like clap's own.
        for line in long.lines() {
            if let Some(at) = line.find("[default: ") {
                assert!(line[..at].trim().is_empty(), "inline default: {line:?}");
            }
        }
        // In -h the computed defaults stay inline.
        assert!(
            short
                .lines()
                .any(|l| l.contains("--rate") && l.contains("[default: unlimited]")),
            "{short}"
        );
    }
}
