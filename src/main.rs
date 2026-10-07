use color_eyre::eyre::{Result, WrapErr};
use echosrv::http::{DEFAULT_MAX_BODY_SIZE, HttpConfig, HttpEchoServer};
use echosrv::network::FdInheritanceConfig;
use echosrv::tcp::TcpConfig;
use echosrv::udp::UdpConfig;
use echosrv::unix::{UnixDatagramConfig, UnixStreamConfig};
use echosrv::{
    EchoServerTrait, TcpEchoServer, UdpEchoServer, UnixDatagramEchoServer, UnixStreamEchoServer,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

const DEFAULT_PORT: u16 = 8080;
const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEFAULT_STREAM_PATH: &str = "/tmp/echosrv_stream.sock";
const DEFAULT_DATAGRAM_PATH: &str = "/tmp/echosrv_datagram.sock";

/// Supported protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Tcp,
    Udp,
    Http,
    UnixStream,
    UnixDatagram,
}

impl Protocol {
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "tcp" => Some(Self::Tcp),
            "udp" => Some(Self::Udp),
            "http" => Some(Self::Http),
            "unix-stream" => Some(Self::UnixStream),
            "unix-dgram" | "unix-datagram" => Some(Self::UnixDatagram),
            _ => None,
        }
    }

    /// Name used to look up an inherited socket (systemd `FileDescriptorName=`).
    fn service_name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Http => "http",
            Self::UnixStream => "unix-stream",
            Self::UnixDatagram => "unix-datagram",
        }
    }

    fn is_unix(self) -> bool {
        matches!(self, Self::UnixStream | Self::UnixDatagram)
    }
}

/// Parsed command line.
#[derive(Debug)]
struct Cli {
    protocol: Protocol,
    host: IpAddr,
    port: u16,
    socket_path: Option<PathBuf>,
}

#[derive(Debug)]
enum Command {
    Run(Cli),
    Help,
    Version,
}

fn usage(program: &str) -> String {
    format!(
        "\
echosrv {version} - async echo server

Usage: {program} [OPTIONS] [PROTOCOL] [PORT | SOCKET_PATH]

Protocols:
  tcp            TCP echo server (default)
  udp            UDP echo server
  http           HTTP echo server (echoes POST bodies)
  unix-stream    Unix domain stream socket server
  unix-dgram     Unix domain datagram socket server (alias: unix-datagram)

Arguments:
  PORT           Port for tcp/udp/http (default: {DEFAULT_PORT})
  SOCKET_PATH    Socket path for unix-stream/unix-dgram
                 (default: {DEFAULT_STREAM_PATH} / {DEFAULT_DATAGRAM_PATH})

Options:
  --host <ADDR>  IP address to bind for tcp/udp/http (default: {DEFAULT_HOST})
  -h, --help     Print this help and exit
  -V, --version  Print version and exit

Environment:
  RUST_LOG       Log filter (default: echosrv=info)
  LISTEN_PID, LISTEN_FDS, LISTEN_FDNAMES
                 systemd socket activation. When present, the server uses the
                 inherited socket named after the protocol (e.g. \"tcp\"), or the
                 only socket passed, instead of binding.

The server stops gracefully on SIGINT (Ctrl-C) or SIGTERM.

Examples:
  {program} tcp 8080
  {program} --host 0.0.0.0 udp 9090
  {program} http
  {program} unix-stream /tmp/echo.sock
",
        version = env!("CARGO_PKG_VERSION"),
    )
}

fn parse_args(args: &[String]) -> std::result::Result<Command, String> {
    let mut host = None;
    let mut positional = Vec::new();
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--host" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--host requires a value".to_string())?;
                host = Some(parse_host(value)?);
            }
            other if other.starts_with("--host=") => {
                host = Some(parse_host(&other["--host=".len()..])?);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option '{other}'"));
            }
            other => positional.push(other),
        }
    }

    if positional.len() > 2 {
        return Err(format!("unexpected argument '{}'", positional[2]));
    }

    let protocol = match positional.first() {
        Some(name) => Protocol::parse(name).ok_or_else(|| format!("unknown protocol '{name}'"))?,
        None => Protocol::Tcp,
    };

    let mut cli = Cli {
        protocol,
        host: host.unwrap_or(DEFAULT_HOST),
        port: DEFAULT_PORT,
        socket_path: None,
    };

    if protocol.is_unix() {
        if host.is_some() {
            return Err("--host cannot be used with Unix domain sockets".to_string());
        }
        cli.socket_path = positional.get(1).map(PathBuf::from);
    } else if let Some(port) = positional.get(1) {
        cli.port = port
            .parse()
            .map_err(|_| format!("invalid port '{port}' (expected 0-65535)"))?;
    }

    Ok(Command::Run(cli))
}

fn parse_host(value: &str) -> std::result::Result<IpAddr, String> {
    let trimmed = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    trimmed
        .parse()
        .map_err(|_| format!("invalid host address '{value}'"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let program = args
        .first()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "echosrv".to_string());

    let cli = match parse_args(args.get(1..).unwrap_or_default()) {
        Ok(Command::Run(cli)) => cli,
        Ok(Command::Help) => {
            print!("{}", usage(&program));
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("echosrv {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("Run '{program} --help' for usage.");
            return ExitCode::FAILURE;
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

fn run(cli: Cli) -> Result<()> {
    color_eyre::install()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("echosrv=info")),
        )
        .init();

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

    match cli.protocol {
        Protocol::Tcp => {
            let config = TcpConfig {
                bind_addr,
                max_connections: 1000,
                buffer_size: 1024,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                ..Default::default()
            }
            .with_fd_inheritance(service_name);
            info!(address = %config.bind_addr, max_connections = config.max_connections, "Starting TCP echo server");
            serve(TcpEchoServer::new(config.into()))
                .await
                .wrap_err("Failed to run TCP echo server")
        }
        Protocol::Udp => {
            let config = UdpConfig {
                bind_addr,
                ..Default::default()
            }
            .with_fd_inheritance(service_name);
            info!(address = %config.bind_addr, "Starting UDP echo server");
            serve(UdpEchoServer::new(config.into()))
                .await
                .wrap_err("Failed to run UDP echo server")
        }
        Protocol::Http => {
            let config = HttpConfig {
                bind_addr,
                max_connections: 1000,
                buffer_size: 8192,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                max_body_size: DEFAULT_MAX_BODY_SIZE,
                ..Default::default()
            }
            .with_fd_inheritance(service_name);
            info!(address = %config.bind_addr, max_connections = config.max_connections, "Starting HTTP echo server");
            serve(HttpEchoServer::new(config))
                .await
                .wrap_err("Failed to run HTTP echo server")
        }
        Protocol::UnixStream => {
            let path = cli
                .socket_path
                .unwrap_or_else(|| PathBuf::from(DEFAULT_STREAM_PATH));
            let config =
                UnixStreamConfig::default().with_fd_inheritance(service_name, path.clone());
            info!(socket_path = %path.display(), max_connections = config.max_connections, "Starting Unix domain stream echo server");
            serve(UnixStreamEchoServer::new(config))
                .await
                .wrap_err("Failed to run Unix domain stream echo server")
        }
        Protocol::UnixDatagram => {
            let path = cli
                .socket_path
                .unwrap_or_else(|| PathBuf::from(DEFAULT_DATAGRAM_PATH));
            let config =
                UnixDatagramConfig::default().with_fd_inheritance(service_name, path.clone());
            info!(socket_path = %path.display(), "Starting Unix domain datagram echo server");
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
}
