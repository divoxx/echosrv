// File descriptor inheritance support for zero-downtime reloads
//
// This module enables echo servers to inherit listening sockets from parent processes
// (like systemd, process managers, or custom init systems) for seamless service restarts
// without dropping active connections.
//
// The inheritance mechanism works by:
// 1. Parent process creates and binds listening sockets
// 2. Parent spawns child process, passing socket file descriptors
// 3. Child process converts inherited FDs back to Tokio listeners/sockets
// 4. Parent can safely exit while child continues serving connections
//
// This is commonly used with:
// - systemd socket activation (LISTEN_FDS/LISTEN_FDNAMES env vars)
// - Custom process managers that implement FD passing
// - Blue-green deployment systems for zero-downtime updates

use crate::{EchoError, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::os::unix::io::RawFd;
use std::path::PathBuf;

/// Represents different socket binding targets that can be inherited or created
#[derive(Debug, Clone)]
pub enum BindTarget {
    /// Network socket (TCP/UDP) bound to IP address and port
    Network(SocketAddr),
    /// Unix domain socket bound to filesystem path
    Unix(PathBuf),
}

/// Strategy for socket creation: inherit from parent or bind new socket
#[derive(Debug, Clone)]
pub enum BindStrategy {
    /// Always bind a new socket to the specified target (default behavior)
    /// This is the traditional approach where each process creates its own socket
    Bind(BindTarget),
    
    /// Always inherit the specified file descriptor from parent process
    /// Fails if the FD is invalid or wrong socket type
    Inherit(RawFd),
    
    /// Try to inherit FD first, fall back to binding if inheritance fails
    /// This provides graceful degradation for development vs production environments
    InheritOrBind { 
        /// Explicit FD to inherit (if None, will look up by service name)
        fd: Option<RawFd>, 
        /// Target to bind to if inheritance fails
        fallback_target: BindTarget 
    },
}

/// Configuration for file descriptor inheritance from parent processes
#[derive(Debug, Clone)]
pub struct FdInheritanceConfig {
    /// Map of service names to inherited file descriptors
    /// Service names help identify which FD corresponds to which service
    /// when multiple sockets are passed from parent process
    pub inherited_fds: HashMap<String, RawFd>,
    
    /// Whether FD inheritance detection was successful
    /// If false, all inheritance attempts will fall back to binding
    pub enable_inheritance: bool,
}

impl FdInheritanceConfig {
    /// Parse systemd-style file descriptor passing from environment variables
    /// 
    /// systemd socket activation passes FDs via these environment variables:
    /// - LISTEN_FDS: Number of file descriptors passed (integer)
    /// - LISTEN_FDNAMES: Colon-separated names for each FD (optional)
    /// - LISTEN_PID: PID that should consume the FDs (for validation)
    /// 
    /// The actual file descriptors start at FD 3 (after stdin/stdout/stderr)
    /// and continue sequentially for LISTEN_FDS count.
    /// 
    /// Example environment:
    /// LISTEN_FDS=2
    /// LISTEN_FDNAMES=tcp-echo:udp-echo
    /// Results in: {"tcp-echo": 3, "udp-echo": 4}
    pub fn from_systemd_env() -> Result<Self> {
        let mut config = Self {
            inherited_fds: HashMap::new(),
            enable_inheritance: false,
        };

        // Parse number of file descriptors passed by systemd
        let listen_fds = std::env::var("LISTEN_FDS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        // No FDs passed - return empty config
        if listen_fds == 0 {
            return Ok(config);
        }

        // Validate that FDs are intended for this process
        // systemd sets LISTEN_PID to the target process PID
        if let Ok(listen_pid) = std::env::var("LISTEN_PID") {
            if let Ok(expected_pid) = listen_pid.parse::<u32>() {
                let current_pid = std::process::id();
                if current_pid != expected_pid {
                    // FDs not intended for this process - ignore them
                    return Ok(config);
                }
            }
        }

        config.enable_inheritance = true;

        // Parse optional service names for each FD
        // If not provided, use generic names like "fd_0", "fd_1", etc.
        let fd_names = std::env::var("LISTEN_FDNAMES")
            .unwrap_or_default()
            .split(':')
            .map(|s| s.to_string())
            .collect::<Vec<_>>();

        // systemd passes FDs starting from 3 (after stdin=0, stdout=1, stderr=2)
        const SD_LISTEN_FDS_START: RawFd = 3;
        
        for i in 0..listen_fds {
            let fd = SD_LISTEN_FDS_START + i as RawFd;
            
            // Use provided name or generate default name
            let name = fd_names.get(i as usize)
                .cloned()
                .unwrap_or_else(|| format!("fd_{}", i));
            
            config.inherited_fds.insert(name, fd);
        }

        Ok(config)
    }

    /// Get file descriptor for a specific service name
    /// Returns None if no FD was inherited for this service
    pub fn get_fd(&self, service_name: &str) -> Option<RawFd> {
        self.inherited_fds.get(service_name).copied()
    }

    /// Check if any file descriptors were inherited
    pub fn has_inherited_fds(&self) -> bool {
        self.enable_inheritance && !self.inherited_fds.is_empty()
    }

    /// Get list of all inherited service names
    pub fn inherited_service_names(&self) -> Vec<&str> {
        self.inherited_fds.keys().map(|s| s.as_str()).collect()
    }
}

/// Socket validation utilities for inherited file descriptors
/// 
/// When inheriting FDs from parent processes, we must validate they are:
/// 1. Actually socket file descriptors (not regular files, pipes, etc.)
/// 2. The correct socket type (stream vs datagram)
/// 3. The correct address family (IPv4/IPv6 vs Unix domain)
/// 
/// This prevents runtime errors and provides clear diagnostic messages
/// when the inheritance setup is incorrect.
pub mod validation {
    use super::*;
    use std::os::unix::io::AsRawFd;

    /// Validate that a file descriptor is a socket of the expected type
    /// 
    /// Uses getsockopt() with SO_TYPE to query the socket type from the kernel.
    /// This is more reliable than trying to use the socket and handling errors.
    /// 
    /// # Arguments
    /// * `fd` - File descriptor to validate
    /// * `expected_type` - Expected socket type (SOCK_STREAM, SOCK_DGRAM, etc.)
    pub fn validate_socket_type(fd: RawFd, expected_type: libc::c_int) -> Result<()> {
        let mut socket_type: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        
        // Query socket type from kernel
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,      // Socket-level option
                libc::SO_TYPE,         // Get socket type
                &mut socket_type as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        
        if result != 0 {
            return Err(EchoError::FdInheritance(
                format!("Failed to get socket type for fd {}: {}", fd, 
                        std::io::Error::last_os_error())
            ));
        }

        if socket_type != expected_type {
            let expected_name = match expected_type {
                libc::SOCK_STREAM => "SOCK_STREAM (TCP/Unix stream)",
                libc::SOCK_DGRAM => "SOCK_DGRAM (UDP/Unix datagram)",
                _ => "unknown socket type",
            };
            return Err(EchoError::FdInheritance(
                format!("Inherited FD {} is not a {} socket (got type {})", 
                        fd, expected_name, socket_type)
            ));
        }
        
        Ok(())
    }

    /// Validate that a socket belongs to the expected address family
    /// 
    /// Uses getsockname() to query the socket's bound address and extract
    /// the address family. This distinguishes between:
    /// - AF_INET (IPv4 network sockets)
    /// - AF_INET6 (IPv6 network sockets)  
    /// - AF_UNIX (Unix domain sockets)
    /// 
    /// # Arguments
    /// * `fd` - File descriptor to validate
    /// * `expected_family` - Expected address family (AF_INET, AF_UNIX, etc.)
    pub fn validate_socket_family(fd: RawFd, expected_family: libc::c_int) -> Result<()> {
        // Storage for any socket address type (IPv4, IPv6, Unix)
        let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        
        // Query socket's bound address from kernel
        let result = unsafe {
            libc::getsockname(
                fd,
                &mut addr as *mut _ as *mut libc::sockaddr,
                &mut len,
            )
        };
        
        if result != 0 {
            return Err(EchoError::FdInheritance(
                format!("Failed to get socket address for fd {}: {}", fd,
                        std::io::Error::last_os_error())
            ));
        }

        // Extract address family from the generic sockaddr structure
        let family = unsafe { 
            (*((&addr) as *const _ as *const libc::sockaddr)).sa_family 
        };
        
        if family != expected_family as libc::sa_family_t {
            let expected_name = match expected_family {
                libc::AF_INET => "AF_INET (IPv4)",
                libc::AF_INET6 => "AF_INET6 (IPv6)", 
                libc::AF_UNIX => "AF_UNIX (Unix domain)",
                _ => "unknown address family",
            };
            let actual_name = match family as libc::c_int {
                libc::AF_INET => "AF_INET (IPv4)",
                libc::AF_INET6 => "AF_INET6 (IPv6)",
                libc::AF_UNIX => "AF_UNIX (Unix domain)",
                _ => "unknown address family",
            };
            return Err(EchoError::FdInheritance(
                format!("Inherited FD {} is {} family, expected {}", 
                        fd, actual_name, expected_name)
            ));
        }
        
        Ok(())
    }
}