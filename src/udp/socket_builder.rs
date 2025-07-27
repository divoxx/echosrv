// UDP socket builder with file descriptor inheritance support
//
// This module implements the BuildSocket trait for UDP sockets, providing:
// - Inheritance of UDP sockets from parent processes
// - Validation that inherited FDs are actually UDP datagram sockets
// - Support for both IPv4 and IPv6 inherited sockets
// - Fallback to normal binding when inheritance is not available
//
// UDP sockets are datagram-oriented (SOCK_DGRAM) and can use either IPv4 (AF_INET)
// or IPv6 (AF_INET6) address families. Unlike TCP, UDP sockets don't have a
// separate "listening" state - they can immediately send/receive after binding.

use crate::network::socket_builder::BuildSocket;
use crate::network::fd_inheritance::BindTarget;
use crate::{EchoError, Result};
use std::os::unix::io::{FromRawFd, RawFd};
use tokio::net::UdpSocket;

/// UDP-specific socket builder
/// 
/// This builder handles creation of UDP sockets with support for file descriptor
/// inheritance from parent processes. It validates that inherited FDs are UDP
/// datagram sockets bound to IP addresses.
pub struct UdpSocketBuilder;

impl BuildSocket<UdpSocket> for UdpSocketBuilder {
    /// UDP uses datagram sockets for connectionless, packet-based communication
    const SOCKET_TYPE: libc::c_int = libc::SOCK_DGRAM;
    
    /// UDP supports both IPv4 and IPv6 address families
    /// When inheriting FDs, we accept either family for maximum flexibility
    const VALID_FAMILIES: &'static [libc::c_int] = &[libc::AF_INET, libc::AF_INET6];
    
    /// Convert inherited file descriptor to Tokio UdpSocket
    /// 
    /// This method assumes the FD has been validated as a UDP datagram socket.
    /// It performs the conversion from raw FD to std::net::UdpSocket, then
    /// to Tokio's async UdpSocket.
    /// 
    /// # Safety
    /// The file descriptor must be:
    /// - A valid socket file descriptor
    /// - A UDP datagram socket (SOCK_DGRAM)
    /// - Bound to an IP address (IPv4 or IPv6)
    /// 
    /// Note: Unlike TCP, UDP sockets don't need to be in "listening" state
    /// since UDP is connectionless and can immediately send/receive.
    /// 
    /// # Arguments
    /// * `fd` - Raw file descriptor inherited from parent process
    fn from_fd(fd: RawFd) -> Result<UdpSocket> {
        // Safety: FD has been validated by validate_inherited_fd()
        // Convert raw FD to std library UdpSocket
        let std_socket = unsafe { std::net::UdpSocket::from_raw_fd(fd) };
        
        // Configure for async operation with Tokio
        // Tokio requires non-blocking sockets for proper async behavior
        std_socket.set_nonblocking(true)
            .map_err(|e| EchoError::Udp(e))?;
        
        // Convert std UdpSocket to Tokio UdpSocket
        // This registers the socket with Tokio's async runtime for efficient I/O
        UdpSocket::from_std(std_socket)
            .map_err(|e| EchoError::Udp(e))
    }
    
    /// Create UDP socket by binding to network address
    /// 
    /// This method handles normal socket creation when inheritance is not
    /// available or not desired. It validates that the target is a network
    /// address (not a Unix domain socket path).
    /// 
    /// # Arguments
    /// * `target` - Where to bind the socket (must be network address)
    fn bind_to(target: &BindTarget) -> Result<UdpSocket> {
        match target {
            // Bind to IP address and port
            BindTarget::Network(addr) => {
                // Create standard library UdpSocket bound to address
                let std_socket = std::net::UdpSocket::bind(addr)
                    .map_err(|e| EchoError::Udp(e))?;
                
                // Configure for async operation
                std_socket.set_nonblocking(true)
                    .map_err(|e| EchoError::Udp(e))?;
                
                // Convert to Tokio async UdpSocket
                UdpSocket::from_std(std_socket)
                    .map_err(|e| EchoError::Udp(e))
            }
            
            // UDP sockets cannot bind to Unix domain socket paths
            BindTarget::Unix(_path) => {
                Err(EchoError::Config(
                    "UDP sockets cannot bind to Unix domain socket paths. Use UnixDatagramSocketBuilder for Unix domain sockets.".into()
                ))
            }
        }
    }
}