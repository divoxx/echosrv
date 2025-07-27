// TCP socket builder with file descriptor inheritance support
//
// This module implements the BuildSocket trait for TCP sockets, providing:
// - Inheritance of TCP listening sockets from parent processes
// - Validation that inherited FDs are actually TCP stream sockets
// - Support for both IPv4 and IPv6 inherited sockets
// - Fallback to normal binding when inheritance is not available
//
// TCP sockets are stream-oriented (SOCK_STREAM) and can use either IPv4 (AF_INET)
// or IPv6 (AF_INET6) address families. When inheriting FDs, we validate both
// the socket type and address family to ensure compatibility.

use crate::network::socket_builder::BuildSocket;
use crate::network::fd_inheritance::BindTarget;
use crate::{EchoError, Result};
use std::os::unix::io::{FromRawFd, RawFd};
use tokio::net::TcpListener;

/// TCP-specific socket builder
/// 
/// This builder handles creation of TCP listening sockets with support for
/// file descriptor inheritance from parent processes. It validates that
/// inherited FDs are TCP stream sockets bound to IP addresses.
pub struct TcpSocketBuilder;

impl BuildSocket<TcpListener> for TcpSocketBuilder {
    /// TCP uses stream sockets for reliable, ordered data delivery
    const SOCKET_TYPE: libc::c_int = libc::SOCK_STREAM;
    
    /// TCP supports both IPv4 and IPv6 address families
    /// When inheriting FDs, we accept either family for maximum flexibility
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_INET, libc::AF_INET6];
    
    /// Convert inherited file descriptor to Tokio TcpListener
    /// 
    /// This method assumes the FD has been validated as a TCP stream socket.
    /// It performs the conversion from raw FD to std::net::TcpListener, then
    /// to Tokio's async TcpListener.
    /// 
    /// # Safety
    /// The file descriptor must be:
    /// - A valid socket file descriptor
    /// - A TCP stream socket (SOCK_STREAM)  
    /// - Bound to an IP address (IPv4 or IPv6)
    /// - In listening state (listen() already called)
    /// 
    /// # Arguments
    /// * `fd` - Raw file descriptor inherited from parent process
    fn from_fd(fd: RawFd) -> Result<TcpListener> {
        // Safety: FD has been validated by validate_inherited_fd()
        // Convert raw FD to std library TcpListener
        let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
        
        // Configure for async operation with Tokio
        // Tokio requires non-blocking sockets for proper async behavior
        std_listener.set_nonblocking(true)
            .map_err(|e| EchoError::Tcp(e))?;
        
        // Convert std TcpListener to Tokio TcpListener
        // This registers the socket with Tokio's async runtime
        TcpListener::from_std(std_listener)
            .map_err(|e| EchoError::Tcp(e))
    }
    
    /// Create TCP listener by binding to network address
    /// 
    /// This method handles normal socket creation when inheritance is not
    /// available or not desired. It validates that the target is a network
    /// address (not a Unix domain socket path).
    /// 
    /// # Arguments
    /// * `target` - Where to bind the socket (must be network address)
    fn bind_to(target: &BindTarget) -> Result<TcpListener> {
        match target {
            // Bind to IP address and port
            BindTarget::Network(addr) => {
                // Create standard library TcpListener bound to address
                let std_listener = std::net::TcpListener::bind(addr)
                    .map_err(|e| EchoError::Tcp(e))?;
                
                // Configure for async operation
                std_listener.set_nonblocking(true)
                    .map_err(|e| EchoError::Tcp(e))?;
                
                // Convert to Tokio async TcpListener
                TcpListener::from_std(std_listener)
                    .map_err(|e| EchoError::Tcp(e))
            }
            
            // TCP sockets cannot bind to Unix domain socket paths
            BindTarget::Unix(_path) => {
                Err(EchoError::Config(
                    "TCP sockets cannot bind to Unix domain socket paths. Use UnixStreamSocketBuilder for Unix domain sockets.".into()
                ))
            }
        }
    }
}