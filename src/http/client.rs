use crate::stream::StreamEchoClient;
use super::protocol::HttpProtocol;

/// HTTP echo client that connects to HTTP echo servers
///
/// This is a type alias for `StreamEchoClient<HttpProtocol>`.
/// It sends HTTP requests and receives HTTP responses.
///
/// # Examples
///
/// Basic client usage:
///
/// ```no_run
/// use echosrv::http::HttpEchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = HttpEchoClient::connect(addr).await?;
///     
///     let response = client.echo_string("Hello, HTTP Server!").await?;
///     println!("Server echoed: {}", response);
///     Ok(())
/// }
/// ```
///
/// Sending binary data:
///
/// ```no_run
/// use echosrv::http::HttpEchoClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let addr = "127.0.0.1:8080".parse()?;
///     let mut client = HttpEchoClient::connect(addr).await?;
///     
///     let data = b"Binary data";
///     let response = client.echo(data).await?;
///     println!("Server echoed {} bytes", response.len());
///     Ok(())
/// }
/// ```
pub type HttpEchoClient = StreamEchoClient<HttpProtocol>; 