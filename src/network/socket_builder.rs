// Generic socket building infrastructure for FD inheritance
//
// This module provides the core abstractions for creating sockets that can either
// be inherited from parent processes or bound fresh. The design uses traits to
// allow protocol-specific implementations while sharing common inheritance logic.
//
// The key insight is that socket creation follows a common pattern:
// 1. Determine socket source (inherit FD vs bind new)
// 2. Validate inherited FDs match expected socket type/family
// 3. Convert raw FDs to appropriate Tokio socket types
// 4. Handle fallback to binding when inheritance fails
//
// This abstraction eliminates code duplication across TCP, UDP, and Unix protocols
// while maintaining type safety and clear error messages.

use crate::network::fd_inheritance::{validation, FdInheritanceConfig, BindStrategy};

// Re-export types that builders need
pub use crate::network::fd_inheritance::BindTarget;
use crate::{EchoError, Result};
use std::os::unix::io::{FromRawFd, RawFd};

/// Generic socket builder that handles FD inheritance logic
/// 
/// This struct provides shared functionality for all socket types.
/// The type parameter T represents the final socket type (TcpListener, UdpSocket, etc.)
pub struct SocketBuilder<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> SocketBuilder<T> {
    /// Resolve binding strategy into concrete socket source
    /// 
    /// This method encapsulates the inheritance logic that's common to all socket types:
    /// - Handle explicit FD inheritance
    /// - Look up FD by service name in inheritance config
    /// - Fall back to binding when inheritance isn't available
    /// 
    /// # Arguments
    /// * `strategy` - How to create the socket (bind, inherit, or try both)
    /// * `service_name` - Name to look up in FD inheritance config
    /// * `fd_config` - Configuration containing inherited FDs from parent process
    pub fn resolve_fd(
        strategy: &BindStrategy,
        service_name: &str,
        fd_config: &FdInheritanceConfig,
    ) -> SocketSource {
        match strategy {
            // Always bind new socket - ignore any inherited FDs
            BindStrategy::Bind(target) => {
                SocketSource::Bind(target.clone())
            }
            
            // Always use specific inherited FD - fail if FD is invalid
            BindStrategy::Inherit(fd) => {
                SocketSource::Inherit(*fd)
            }
            
            // Try inheritance first, fall back to binding
            // This provides the most flexible behavior for different deployment scenarios
            BindStrategy::InheritOrBind { fd, fallback_target } => {
                // Check explicit FD first
                if let Some(fd) = fd {
                    SocketSource::Inherit(*fd)
                } 
                // Check if service name maps to inherited FD
                else if let Some(inherited_fd) = fd_config.get_fd(service_name) {
                    SocketSource::Inherit(inherited_fd)
                } 
                // No inheritance available - bind new socket
                else {
                    SocketSource::Bind(fallback_target.clone())
                }
            }
        }
    }
}

/// Resolved socket source after inheritance logic is applied
/// 
/// This enum represents the final decision on how to create a socket.
/// It simplifies the subsequent socket creation code by eliminating
/// the need to re-evaluate inheritance logic.
#[derive(Debug, Clone)]
pub enum SocketSource {
    /// Create socket by binding to specified target
    Bind(BindTarget),
    /// Create socket from inherited file descriptor
    Inherit(RawFd),
}

/// Trait for protocol-specific socket building
/// 
/// Each socket type (TCP, UDP, Unix stream, Unix datagram) implements this trait
/// to provide protocol-specific creation logic while sharing the common inheritance
/// framework provided by SocketBuilder.
/// 
/// The trait uses associated constants to declare socket validation requirements,
/// which enables compile-time verification and clear error messages.
pub trait BuildSocket<T> {
    /// Expected socket type for validation (SOCK_STREAM, SOCK_DGRAM)
    /// 
    /// When inheriting FDs, we validate they match this socket type.
    /// This prevents runtime errors from trying to use a TCP FD as UDP socket.
    const SOCKET_TYPE: libc::c_int;
    
    /// Valid address families for this socket type
    /// 
    /// Network sockets accept AF_INET and AF_INET6, while Unix domain sockets
    /// only accept AF_UNIX. This enables flexible validation while maintaining
    /// type safety.
    const VALID_FAMILIES: &'static [libc::c_int];
    
    /// Create socket from inherited file descriptor
    /// 
    /// This method assumes the FD has already been validated by validate_inherited_fd().
    /// It performs the low-level conversion from raw FD to typed Tokio socket.
    /// 
    /// # Safety
    /// The file descriptor must be valid and match the expected socket type/family.
    /// Callers should always call validate_inherited_fd() first.
    /// 
    /// # Arguments
    /// * `fd` - Raw file descriptor inherited from parent process
    fn from_fd(fd: RawFd) -> Result<T>;
    
    /// Create socket by binding to specified target
    /// 
    /// This method handles fresh socket creation when inheritance is not available
    /// or not desired. It should handle target type validation (e.g., TCP sockets
    /// cannot bind to Unix paths).
    /// 
    /// # Arguments
    /// * `target` - Where to bind the socket (network address or Unix path)
    fn bind_to(target: &BindTarget) -> Result<T>;
    
    /// Main entry point for socket creation
    /// 
    /// This method orchestrates the entire socket creation process:
    /// 1. Resolve inheritance strategy to concrete source
    /// 2. Validate inherited FDs if applicable
    /// 3. Create socket using appropriate method
    /// 4. Return typed socket ready for use
    /// 
    /// # Arguments
    /// * `strategy` - How to create the socket (bind, inherit, or fallback)
    /// * `service_name` - Service name for FD lookup in inheritance config
    /// * `fd_config` - Configuration with inherited FDs from parent process
    fn build(
        strategy: &BindStrategy,
        service_name: &str,
        fd_config: &FdInheritanceConfig,
    ) -> Result<T> {
        // Resolve strategy to concrete socket source
        let source = SocketBuilder::<T>::resolve_fd(strategy, service_name, fd_config);
        
        match source {
            SocketSource::Inherit(fd) => {
                // Validate FD matches expected socket type and family
                Self::validate_inherited_fd(fd)?;
                // Convert validated FD to typed socket
                Self::from_fd(fd)
            }
            SocketSource::Bind(target) => {
                // Create fresh socket by binding
                Self::bind_to(&target)
            }
        }
    }
    
    /// Validate inherited file descriptor matches socket requirements
    /// 
    /// This method performs comprehensive validation of inherited FDs:
    /// 1. Verify FD is correct socket type (stream vs datagram)
    /// 2. Verify FD uses compatible address family
    /// 
    /// The validation is protocol-specific but uses shared validation utilities.
    /// Multiple address families can be supported (e.g., IPv4 and IPv6 for TCP).
    /// 
    /// # Arguments
    /// * `fd` - Raw file descriptor to validate
    fn validate_inherited_fd(fd: RawFd) -> Result<()> {
        // Validate socket type (stream vs datagram)
        validation::validate_socket_type(fd, Self::SOCKET_TYPE)?;
        
        // Try each valid address family until one succeeds
        // This allows protocols like TCP to accept both IPv4 and IPv6 sockets
        let mut last_error = None;
        for &family in Self::VALID_FAMILIES {
            match validation::validate_socket_family(fd, family) {
                Ok(()) => return Ok(()), // Found compatible family
                Err(e) => last_error = Some(e), // Keep trying other families
            }
        }
        
        // No valid family found - return the last error for diagnostics
        match last_error {
            Some(err) => Err(err),
            None => Err(EchoError::FdInheritance(
                "No valid socket families configured for this protocol".into()
            )),
        }
    }
}