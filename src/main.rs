use echosrv::{TcpEchoServer, UdpEchoServer, EchoServerTrait};
use echosrv::tcp::TcpConfig;
use echosrv::udp::UdpConfig;
use color_eyre::eyre::{Result, WrapErr};
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize error handling
    color_eyre::install()?;

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("echosrv=info")
        .init();

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    
    // Default to TCP if no protocol specified
    let protocol = args.get(1).map(|s| s.to_lowercase()).unwrap_or_else(|| "tcp".to_string());
    
    let port = args
        .get(2)
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8080);

    match protocol.as_str() {
        "tcp" => {
            let config = TcpConfig {
                bind_addr: format!("127.0.0.1:{}", port).parse().unwrap(),
                max_connections: 1000, // Higher limit for production use
                buffer_size: 1024,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
            };

            info!(address = %config.bind_addr, max_connections = config.max_connections, "Starting TCP echo server");

            let server = TcpEchoServer::new(config.into());
            server.run().await.wrap_err("Failed to run TCP echo server")?;
        }
        "udp" => {
            let config = UdpConfig {
                bind_addr: format!("127.0.0.1:{}", port).parse().unwrap(),
                buffer_size: 1024,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
            };

            info!(address = %config.bind_addr, "Starting UDP echo server");

            let server = UdpEchoServer::new(config.into());
            server.run().await.wrap_err("Failed to run UDP echo server")?;
        }
        _ => {
            eprintln!("Usage: {} [tcp|udp] [port]", args[0]);
            eprintln!("  tcp|udp: Protocol to use (default: tcp)");
            eprintln!("  port:    Port to bind to (default: 8080)");
            eprintln!();
            eprintln!("Examples:");
            eprintln!("  {} tcp 8080    # Start TCP echo server on port 8080", args[0]);
            eprintln!("  {} udp 9090    # Start UDP echo server on port 9090", args[0]);
            eprintln!("  {} tcp         # Start TCP echo server on default port 8080", args[0]);
            std::process::exit(1);
        }
    }

    Ok(())
} 