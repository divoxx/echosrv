use crate::stream::{StreamProtocol, StreamConfig};

use std::io;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, TcpListener};
use async_trait::async_trait;

/// HTTP protocol implementation for echo server
///
/// Only accepts POST requests and echoes the request body back.
/// Returns 405 Method Not Allowed for non-POST requests.
pub struct HttpProtocol;

#[derive(Debug, thiserror::Error)]
pub enum HttpProtocolError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("HTTP parsing error: {0}")]
    HttpParse(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Incomplete request")]
    IncompleteRequest,
}

/// HTTP stream wrapper that handles HTTP framing
pub struct HttpStream {
    inner: TcpStream,
    request_buffer: Vec<u8>,
    body_start: Option<usize>,
    method: Option<String>,
    request_complete: bool,
}

impl HttpStream {
    fn new(stream: TcpStream) -> Self {
        Self {
            inner: stream,
            request_buffer: Vec::new(),
            body_start: None,
            method: None,
            request_complete: false,
        }
    }
}

#[async_trait]
impl StreamProtocol for HttpProtocol {
    type Error = HttpProtocolError;
    type Listener = TcpListener;
    type Stream = HttpStream;

    async fn bind(config: &StreamConfig) -> std::result::Result<Self::Listener, Self::Error> {
        TcpListener::bind(config.bind_addr).await.map_err(HttpProtocolError::Io)
    }

    async fn accept(listener: &mut Self::Listener) -> std::result::Result<(Self::Stream, SocketAddr), Self::Error> {
        let (stream, addr) = listener.accept().await.map_err(HttpProtocolError::Io)?;
        Ok((HttpStream::new(stream), addr))
    }

    async fn connect(addr: SocketAddr) -> std::result::Result<Self::Stream, Self::Error> {
        let stream = TcpStream::connect(addr).await.map_err(HttpProtocolError::Io)?;
        Ok(HttpStream::new(stream))
    }

    async fn read(stream: &mut Self::Stream, buffer: &mut [u8]) -> std::result::Result<usize, Self::Error> {
        // If we've already completed a request, return 0 to indicate end of data
        if stream.request_complete {
            return Ok(0);
        }

        // Read more data into our buffer
        let mut temp_buffer = vec![0u8; 1024];
        let n = stream.inner.read(&mut temp_buffer).await.map_err(HttpProtocolError::Io)?;
        if n == 0 {
            return Err(HttpProtocolError::IncompleteRequest);
        }
        
        stream.request_buffer.extend_from_slice(&temp_buffer[..n]);
        
        // Try to parse headers if we haven't already
        if stream.body_start.is_none() {
            let mut headers = [httparse::EMPTY_HEADER; 32];
            let mut req = httparse::Request::new(&mut headers);
            
            match req.parse(&stream.request_buffer) {
                Ok(httparse::Status::Complete(parsed_len)) => {
                    stream.body_start = Some(parsed_len);
                    
                    // Check method
                    if let Some(method) = req.method {
                        stream.method = Some(method.to_string());
                        
                        if method != "POST" {
                            // Send 405 response immediately
                            let body = format!("Method {} not allowed. Only POST requests are accepted.", method);
                            let response = format!(
                                "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: {}\r\nAllow: POST\r\n\r\n{}",
                                body.len(), body
                            );
                            stream.inner.write_all(response.as_bytes()).await.map_err(HttpProtocolError::Io)?;
                            stream.inner.flush().await.map_err(HttpProtocolError::Io)?;
                            stream.request_complete = true;
                            return Err(HttpProtocolError::InvalidRequest(format!("Method {} not allowed", method)));
                        }
                    }
                }
                Ok(httparse::Status::Partial) => {
                    // Need more data
                    return Err(HttpProtocolError::IncompleteRequest);
                }
                Err(e) => {
                    return Err(HttpProtocolError::HttpParse(format!("Failed to parse headers: {}", e)));
                }
            }
        }
        
        // If we have a body start position, extract body data
        if let Some(body_start) = stream.body_start {
            let available_body = stream.request_buffer.len().saturating_sub(body_start);
            if available_body > 0 {
                let body_data = &stream.request_buffer[body_start..];
                let copy_len = body_data.len().min(buffer.len());
                
                buffer[..copy_len].copy_from_slice(&body_data[..copy_len]);
                
                // Remove the copied data from buffer
                stream.request_buffer.drain(..body_start + copy_len);
                stream.body_start = stream.body_start.map(|start| start.saturating_sub(copy_len));
                
                // Mark request as complete if we've read all the body
                if copy_len == available_body {
                    stream.request_complete = true;
                }
                
                return Ok(copy_len);
            } else {
                // We've read all the body data
                stream.request_complete = true;
                return Ok(0);
            }
        }
        
        // No body data available yet, but we're still reading headers
        Err(HttpProtocolError::IncompleteRequest)
    }

    async fn write(stream: &mut Self::Stream, data: &[u8]) -> std::result::Result<(), Self::Error> {
        // Echo only the body content, no HTTP headers
        stream.inner.write_all(data).await.map_err(HttpProtocolError::Io)?;
        stream.inner.flush().await.map_err(HttpProtocolError::Io)?;
        Ok(())
    }

    async fn flush(stream: &mut Self::Stream) -> std::result::Result<(), Self::Error> {
        stream.inner.flush().await.map_err(HttpProtocolError::Io)
    }

    fn map_io_error(err: io::Error) -> Self::Error {
        HttpProtocolError::Io(err)
    }
}



 