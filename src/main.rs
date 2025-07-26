use echosrv::{TcpEchoServer, UdpEchoServer, UnixStreamEchoServer, UnixDatagramEchoServer, EchoServerTrait};
use echosrv::tcp::TcpConfig;
use echosrv::udp::UdpConfig;
use echosrv::unix::{UnixStreamConfig, UnixDatagramConfig};
use echosrv::http::{HttpConfig, HttpEchoServer};
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
    
    // For Unix domain sockets, the second argument is the socket path
    // For TCP/UDP, it's the port
    let socket_path_or_port = args.get(2);

    match protocol.as_str() {
        "http" => {
            let port = socket_path_or_port
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(8080);
                
            let config = HttpConfig {
                bind_addr: format!("127.0.0.1:{}", port).parse().unwrap(),
                max_connections: 1000, // Higher limit for production use
                buffer_size: 8192, // Larger buffer for HTTP
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
                server_name: Some("EchoServer/1.0".to_string()),
                echo_headers: true,
                default_content_type: Some("text/plain".to_string()),
            };
            let server = HttpEchoServer::new(config.clone().into());
            info!(address = %config.bind_addr, max_connections = config.max_connections, "Starting HTTP echo server");
            server.run().await.wrap_err("Failed to run HTTP echo server")?;
        }
        "tcp" => {
            let port = socket_path_or_port
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(8080);
                
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
            let port = socket_path_or_port
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(8080);
                
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
        "unix-stream" => {
            let socket_path = socket_path_or_port
                .map(|p| p.into())
                .unwrap_or_else(|| "/tmp/echosrv_stream.sock".into());
                
            let config = UnixStreamConfig {
                socket_path,
                max_connections: 1000,
                buffer_size: 1024,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
            };

            info!(socket_path = %config.socket_path.display(), max_connections = config.max_connections, "Starting Unix domain stream echo server");

            let server = UnixStreamEchoServer::new(config);
            server.run().await.wrap_err("Failed to run Unix domain stream echo server")?;
        }
        "unix-dgram" => {
            let socket_path = socket_path_or_port
                .map(|p| p.into())
                .unwrap_or_else(|| "/tmp/echosrv_datagram.sock".into());
                
            let config = UnixDatagramConfig {
                socket_path,
                buffer_size: 1024,
                read_timeout: Duration::from_secs(30),
                write_timeout: Duration::from_secs(30),
            };

            info!(socket_path = %config.socket_path.display(), "Starting Unix domain datagram echo server");

            let server = UnixDatagramEchoServer::new(config);
            server.run().await.wrap_err("Failed to run Unix domain datagram echo server")?;
        }
        _ => {
            eprintln!("Usage: {} [http|tcp|udp|unix-stream|unix-dgram] [port|socket_path]", args[0]);
            eprintln!("  http|tcp|udp|unix-stream|unix-dgram: Protocol to use (default: tcp)");
            eprintln!("  port:    Port to bind to for HTTP/TCP/UDP (default: 8080)");
            eprintln!("  socket_path: Unix domain socket path (default: /tmp/echosrv_*.sock)");
            eprintln!();
            eprintln!("Examples:");
            eprintln!("  {} http 8080                   # Start HTTP echo server on port 8080", args[0]);
            eprintln!("  {} tcp 8080                    # Start TCP echo server on port 8080", args[0]);
            eprintln!("  {} udp 9090                    # Start UDP echo server on port 9090", args[0]);
            eprintln!("  {} unix-stream /tmp/echo.sock   # Start Unix stream server", args[0]);
            eprintln!("  {} unix-dgram /tmp/echo.sock    # Start Unix datagram server", args[0]);
            eprintln!("  {} http                        # Start HTTP echo server on default port 8080", args[0]);
            eprintln!("  {} tcp                         # Start TCP echo server on default port 8080", args[0]);
            std::process::exit(1);
        }
    }

    Ok(())
} 