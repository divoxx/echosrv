use echosrv::tcp::{Config, EchoServer};
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

    // Parse command line arguments for port (optional)
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .get(1)
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8080);

    let config = Config {
        bind_addr: format!("127.0.0.1:{}", port).parse().unwrap(),
        max_connections: 1000, // Higher limit for production use
        buffer_size: 1024,
        read_timeout: Some(Duration::from_secs(30)),
        write_timeout: Some(Duration::from_secs(30)),
    };

    info!(address = %config.bind_addr, max_connections = config.max_connections, "Starting echo server");

    let server = EchoServer::new(config);
    server.run().await.wrap_err("Failed to run echo server")?;

    Ok(())
} 