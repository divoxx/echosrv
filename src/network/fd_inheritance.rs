//! File descriptor inheritance support for zero-downtime reloads and socket activation.
//!
//! This module enables echo servers to inherit listening sockets from a parent
//! process (systemd socket activation, a process manager, a blue/green launcher)
//! instead of binding them themselves.
//!
//! The inheritance mechanism works by:
//! 1. The parent process creates and binds the listening sockets.
//! 2. The parent spawns the server, passing the socket file descriptors.
//! 3. The server converts the inherited FDs into Tokio listeners/sockets.
//!
//! # Ownership model
//!
//! A file descriptor must have exactly one owner, otherwise it would be closed
//! twice. This module enforces that in two places:
//!
//! * [`InheritedFd`] is a cloneable, *take-once* handle: whichever server consumes
//!   it first gets the [`OwnedFd`]; later attempts see it as already consumed.
//! * [`FdInheritanceConfig`] is a shared pool of named FDs. Taking an FD removes
//!   it from the pool. [`FdInheritanceConfig::from_systemd_env`] parses the
//!   environment once per process and always returns the same pool, so two
//!   servers asking for the same service name can never both own the same FD.

use crate::{EchoError, Result};
use std::fmt;
use std::net::SocketAddr;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Represents different socket binding targets that can be inherited or created
#[derive(Debug, Clone)]
pub enum BindTarget {
    /// Network socket (TCP/UDP) bound to IP address and port
    Network(SocketAddr),
    /// Unix domain socket bound to filesystem path
    Unix(PathBuf),
}

/// A take-once handle to a file descriptor handed over by a parent process.
///
/// Cloning an `InheritedFd` shares the same underlying descriptor: the first
/// call to [`take`](Self::take) returns the [`OwnedFd`], every later call
/// returns `None`. This lets configuration structs stay [`Clone`] while
/// guaranteeing that a descriptor is owned (and therefore closed) exactly once.
///
/// # Constructing
///
/// * From an [`OwnedFd`] (safe): `InheritedFd::new(owned)` or `owned.into()`.
/// * From a raw descriptor number (unsafe): [`FromRawFd::from_raw_fd`]. The
///   caller must guarantee that the descriptor is open and that nothing else in
///   the process owns or will close it — the same contract as
///   [`OwnedFd::from_raw_fd`].
///
/// # Examples
///
/// ```
/// use echosrv::network::InheritedFd;
/// use std::os::fd::OwnedFd;
///
/// let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
/// let fd = InheritedFd::new(OwnedFd::from(listener));
/// let clone = fd.clone();
/// assert!(fd.take().is_some());
/// assert!(clone.take().is_none()); // already consumed
/// ```
#[derive(Clone)]
pub struct InheritedFd {
    raw: RawFd,
    slot: Arc<Mutex<Option<OwnedFd>>>,
}

impl InheritedFd {
    /// Wraps an owned file descriptor.
    pub fn new(fd: OwnedFd) -> Self {
        Self {
            raw: fd.as_raw_fd(),
            slot: Arc::new(Mutex::new(Some(fd))),
        }
    }

    /// The raw descriptor number (for diagnostics only; ownership is not transferred).
    pub fn raw_fd(&self) -> RawFd {
        self.raw
    }

    /// Takes ownership of the descriptor. Returns `None` if it was already taken.
    pub fn take(&self) -> Option<OwnedFd> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Returns `true` if the descriptor has already been taken.
    pub fn is_consumed(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
    }
}

impl From<OwnedFd> for InheritedFd {
    fn from(fd: OwnedFd) -> Self {
        Self::new(fd)
    }
}

impl FromRawFd for InheritedFd {
    /// Takes ownership of a raw descriptor.
    ///
    /// # Safety
    ///
    /// `fd` must be an open file descriptor that is not owned by anything else
    /// in the process (see [`OwnedFd::from_raw_fd`]).
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        // SAFETY: forwarded to the caller's contract.
        Self::new(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

impl fmt::Debug for InheritedFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InheritedFd")
            .field("fd", &self.raw)
            .field("consumed", &self.is_consumed())
            .finish()
    }
}

/// Strategy for socket creation: inherit from parent or bind new socket
#[derive(Debug, Clone)]
pub enum BindStrategy {
    /// Always bind a new socket to the specified target (default behavior)
    Bind(BindTarget),

    /// Always use the given inherited file descriptor.
    ///
    /// Fails if the descriptor was already consumed, is not a socket, or has the
    /// wrong type/family for the protocol.
    Inherit(InheritedFd),

    /// Try to inherit first, fall back to binding if no descriptor is available.
    ///
    /// The explicit `fd` is tried first; if it is `None` (or already consumed),
    /// the server's service name is looked up in the [`FdInheritanceConfig`]
    /// (e.g. systemd's `LISTEN_FDNAMES`). If nothing is found, `fallback_target`
    /// is bound.
    InheritOrBind {
        /// Explicit FD to inherit (if None, will look up by service name)
        fd: Option<InheritedFd>,
        /// Target to bind to if inheritance fails
        fallback_target: BindTarget,
    },
}

impl BindStrategy {
    /// The target this strategy would bind to (either the `Bind` target or the
    /// `InheritOrBind` fallback). Returns `None` for [`BindStrategy::Inherit`].
    pub fn bind_target(&self) -> Option<&BindTarget> {
        match self {
            BindStrategy::Bind(target) => Some(target),
            BindStrategy::InheritOrBind {
                fallback_target, ..
            } => Some(fallback_target),
            BindStrategy::Inherit(_) => None,
        }
    }
}

struct NamedFd {
    name: String,
    fd: OwnedFd,
}

/// A shared pool of named file descriptors inherited from a parent process.
///
/// Cloning the config shares the pool. Taking a descriptor removes it, so each
/// descriptor is handed to at most one server.
#[derive(Clone, Default)]
pub struct FdInheritanceConfig {
    pool: Arc<Mutex<Vec<NamedFd>>>,
}

/// systemd passes FDs starting from 3 (after stdin=0, stdout=1, stderr=2)
const SD_LISTEN_FDS_START: RawFd = 3;

/// Name systemd uses for descriptors without an explicit name.
pub const SYSTEMD_UNNAMED_FD: &str = "unknown";

static SYSTEMD_POOL: OnceLock<FdInheritanceConfig> = OnceLock::new();

impl FdInheritanceConfig {
    /// Creates an empty pool (no inherited descriptors).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Creates a pool from explicitly provided descriptors.
    ///
    /// Use this for custom process managers that pass descriptors without the
    /// systemd environment protocol.
    pub fn from_fds<I>(fds: I) -> Self
    where
        I: IntoIterator<Item = (String, OwnedFd)>,
    {
        let pool = fds
            .into_iter()
            .map(|(name, fd)| NamedFd { name, fd })
            .collect();
        Self {
            pool: Arc::new(Mutex::new(pool)),
        }
    }

    /// Returns the process-wide pool of descriptors passed via systemd socket
    /// activation (`LISTEN_PID`, `LISTEN_FDS`, `LISTEN_FDNAMES`).
    ///
    /// The environment is parsed only once per process; every call returns a
    /// handle to the same pool, so descriptors are never owned twice.
    ///
    /// Rules (matching `sd_listen_fds_with_names(3)`):
    /// * `LISTEN_PID` must be present and equal to the current PID, otherwise
    ///   no descriptors are inherited.
    /// * Descriptors start at 3 and are numbered consecutively for `LISTEN_FDS`.
    /// * Names come from the colon-separated `LISTEN_FDNAMES`; missing names
    ///   default to [`SYSTEMD_UNNAMED_FD`] (`"unknown"`).
    /// * `FD_CLOEXEC` is set on every inherited descriptor so they do not leak
    ///   into child processes.
    ///
    /// The `LISTEN_*` variables are *not* removed here: mutating the environment
    /// is unsafe once other threads may be running (e.g. inside a Tokio
    /// runtime). Since the descriptors are `FD_CLOEXEC` and `LISTEN_PID` will not
    /// match any child, leaving them set is harmless. Binaries that want to
    /// unset them should do so early in `main`, before spawning threads.
    ///
    /// Malformed variables are logged and treated as "no descriptors".
    pub fn from_systemd_env() -> Result<Self> {
        Ok(SYSTEMD_POOL.get_or_init(Self::load_systemd_env).clone())
    }

    fn load_systemd_env() -> Self {
        let pid = std::env::var("LISTEN_PID").ok();
        let fds = std::env::var("LISTEN_FDS").ok();
        let names = std::env::var("LISTEN_FDNAMES").ok();

        let parsed = match parse_systemd_env(
            pid.as_deref(),
            fds.as_deref(),
            names.as_deref(),
            std::process::id(),
        ) {
            Ok(parsed) => parsed,
            Err(msg) => {
                tracing::warn!(error = %msg, "Ignoring malformed systemd socket activation environment");
                return Self::empty();
            }
        };

        let mut pool = Vec::with_capacity(parsed.len());
        for (name, raw) in parsed {
            // Mark close-on-exec; this also checks that the descriptor is open.
            if let Err(e) = set_cloexec(raw) {
                tracing::warn!(fd = raw, name = %name, error = %e, "Skipping invalid inherited file descriptor");
                continue;
            }
            // SAFETY: systemd hands these descriptors to this process (LISTEN_PID
            // matched), they are open (fcntl succeeded above), and this function
            // runs at most once per process (guarded by `SYSTEMD_POOL`), so no
            // other `OwnedFd` for them is ever created by this crate.
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            pool.push(NamedFd { name, fd });
        }

        Self {
            pool: Arc::new(Mutex::new(pool)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<NamedFd>> {
        self.pool
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Takes the first descriptor named `service_name` out of the pool.
    pub fn take(&self, service_name: &str) -> Option<OwnedFd> {
        let mut pool = self.lock();
        let index = pool.iter().position(|entry| entry.name == service_name)?;
        Some(pool.remove(index).fd)
    }

    /// Takes the descriptor named `service_name`; if there is none and the pool
    /// holds exactly one descriptor, takes that one regardless of its name.
    ///
    /// This mirrors the common systemd setup where a single `.socket` unit
    /// passes one descriptor named after the unit (or `"unknown"`).
    pub fn take_named_or_sole(&self, service_name: &str) -> Option<OwnedFd> {
        let mut pool = self.lock();
        if let Some(index) = pool.iter().position(|entry| entry.name == service_name) {
            return Some(pool.remove(index).fd);
        }
        if pool.len() == 1 {
            return pool.pop().map(|entry| entry.fd);
        }
        None
    }

    /// Returns the raw descriptor number for `service_name` without taking it.
    pub fn get_fd(&self, service_name: &str) -> Option<RawFd> {
        self.lock()
            .iter()
            .find(|entry| entry.name == service_name)
            .map(|entry| entry.fd.as_raw_fd())
    }

    /// Returns `true` if any descriptors remain in the pool.
    pub fn has_inherited_fds(&self) -> bool {
        !self.lock().is_empty()
    }

    /// Names of the descriptors remaining in the pool (may contain duplicates).
    pub fn inherited_service_names(&self) -> Vec<String> {
        self.lock().iter().map(|entry| entry.name.clone()).collect()
    }
}

impl fmt::Debug for FdInheritanceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pool = self.lock();
        f.debug_map()
            .entries(pool.iter().map(|entry| (&entry.name, entry.fd.as_raw_fd())))
            .finish()
    }
}

/// Parses the systemd socket activation variables into `(name, fd)` pairs.
///
/// Pure function so it can be tested without touching the process environment.
pub(crate) fn parse_systemd_env(
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    listen_fdnames: Option<&str>,
    current_pid: u32,
) -> std::result::Result<Vec<(String, RawFd)>, String> {
    // LISTEN_PID is mandatory: without it we cannot tell whether the FDs are ours.
    let Some(pid) = listen_pid else {
        return Ok(Vec::new());
    };
    let pid: u32 = pid
        .trim()
        .parse()
        .map_err(|e| format!("invalid LISTEN_PID {pid:?}: {e}"))?;
    if pid != current_pid {
        return Ok(Vec::new());
    }

    let Some(count) = listen_fds else {
        return Ok(Vec::new());
    };
    let count: u16 = count
        .trim()
        .parse()
        .map_err(|e| format!("invalid LISTEN_FDS {count:?}: {e}"))?;

    let names: Vec<&str> = match listen_fdnames {
        Some(names) if !names.is_empty() => names.split(':').collect(),
        _ => Vec::new(),
    };

    Ok((0..count)
        .map(|i| {
            let name = names
                .get(usize::from(i))
                .filter(|name| !name.is_empty())
                .map_or_else(|| SYSTEMD_UNNAMED_FD.to_string(), |name| name.to_string());
            (name, SD_LISTEN_FDS_START + RawFd::from(i))
        })
        .collect())
}

/// Sets `FD_CLOEXEC` on `fd`. Fails with `EBADF` if the descriptor is not open.
fn set_cloexec(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl with F_GETFD/F_SETFD only reads/writes descriptor flags and
    // reports EBADF for invalid descriptors; no memory is accessed.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Socket validation utilities for inherited file descriptors
///
/// When inheriting FDs from parent processes, we must validate they are:
/// 1. Actually socket file descriptors (not regular files, pipes, etc.)
/// 2. The correct socket type (stream vs datagram)
/// 3. The correct address family (IPv4/IPv6 vs Unix domain)
/// 4. For stream servers, already listening
///
/// This prevents runtime errors and provides clear diagnostic messages
/// when the inheritance setup is incorrect.
pub mod validation {
    use super::*;

    fn getsockopt_int(fd: RawFd, option: libc::c_int) -> std::io::Result<libc::c_int> {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `value`/`len` are valid, correctly sized out-pointers for an
        // int-valued SOL_SOCKET option; an invalid fd just yields an error.
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                &mut value as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(value)
    }

    /// Validate that a file descriptor is a socket of the expected type
    ///
    /// Uses `getsockopt(SO_TYPE)` to query the socket type from the kernel.
    ///
    /// # Arguments
    /// * `fd` - File descriptor to validate
    /// * `expected_type` - Expected socket type (`SOCK_STREAM`, `SOCK_DGRAM`, etc.)
    pub fn validate_socket_type(fd: impl AsFd, expected_type: libc::c_int) -> Result<()> {
        let fd = fd.as_fd().as_raw_fd();
        let socket_type = getsockopt_int(fd, libc::SO_TYPE).map_err(|e| {
            EchoError::FdInheritance(format!("Failed to get socket type for fd {fd}: {e}"))
        })?;

        if socket_type != expected_type {
            let expected_name = match expected_type {
                libc::SOCK_STREAM => "SOCK_STREAM (TCP/Unix stream)",
                libc::SOCK_DGRAM => "SOCK_DGRAM (UDP/Unix datagram)",
                _ => "unknown socket type",
            };
            return Err(EchoError::FdInheritance(format!(
                "Inherited FD {fd} is not a {expected_name} socket (got type {socket_type})"
            )));
        }

        Ok(())
    }

    /// Validate that a stream socket is in the listening state (`SO_ACCEPTCONN`).
    ///
    /// Platforms that cannot report `SO_ACCEPTCONN` via `getsockopt` (e.g.
    /// macOS returns `ENOPROTOOPT`) skip this check.
    pub fn validate_listening(fd: impl AsFd) -> Result<()> {
        let fd = fd.as_fd().as_raw_fd();
        let listening = match getsockopt_int(fd, libc::SO_ACCEPTCONN) {
            Ok(value) => value,
            Err(e) if e.raw_os_error() == Some(libc::ENOPROTOOPT) => {
                tracing::debug!(fd, "SO_ACCEPTCONN not supported; skipping listening check");
                return Ok(());
            }
            Err(e) => {
                return Err(EchoError::FdInheritance(format!(
                    "Failed to query SO_ACCEPTCONN for fd {fd}: {e}"
                )));
            }
        };
        if listening == 0 {
            return Err(EchoError::FdInheritance(format!(
                "Inherited FD {fd} is not a listening socket (listen() was not called)"
            )));
        }
        Ok(())
    }

    /// Validate that a socket belongs to the expected address family
    ///
    /// Uses `getsockname()` to query the socket's address family:
    /// `AF_INET` (IPv4), `AF_INET6` (IPv6) or `AF_UNIX` (Unix domain).
    ///
    /// # Arguments
    /// * `fd` - File descriptor to validate
    /// * `expected_family` - Expected address family (`AF_INET`, `AF_UNIX`, etc.)
    pub fn validate_socket_family(fd: impl AsFd, expected_family: libc::c_int) -> Result<()> {
        let fd = fd.as_fd().as_raw_fd();
        // SAFETY: sockaddr_storage is plain old data; all-zero is a valid value.
        let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

        // SAFETY: `addr` is large enough for any socket address and `len`
        // holds its size, as getsockname requires.
        let result =
            unsafe { libc::getsockname(fd, &mut addr as *mut _ as *mut libc::sockaddr, &mut len) };

        if result != 0 {
            return Err(EchoError::FdInheritance(format!(
                "Failed to get socket address for fd {}: {}",
                fd,
                std::io::Error::last_os_error()
            )));
        }

        let family = addr.ss_family;

        if family != expected_family as libc::sa_family_t {
            let name = |family: libc::c_int| match family {
                libc::AF_INET => "AF_INET (IPv4)",
                libc::AF_INET6 => "AF_INET6 (IPv6)",
                libc::AF_UNIX => "AF_UNIX (Unix domain)",
                _ => "unknown address family",
            };
            return Err(EchoError::FdInheritance(format!(
                "Inherited FD {} is {} family, expected {}",
                fd,
                name(family as libc::c_int),
                name(expected_family)
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PID: u32 = 4242;

    #[test]
    fn parse_requires_listen_pid() {
        assert!(
            parse_systemd_env(None, Some("2"), None, PID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parse_ignores_other_pid() {
        assert!(
            parse_systemd_env(Some("1"), Some("2"), None, PID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_systemd_env(Some("abc"), Some("1"), None, PID).is_err());
        assert!(parse_systemd_env(Some("4242"), Some("x"), None, PID).is_err());
    }

    #[test]
    fn parse_names_and_defaults() {
        let parsed = parse_systemd_env(Some("4242"), Some("3"), Some("tcp:"), PID).unwrap();
        assert_eq!(
            parsed,
            vec![
                ("tcp".to_string(), 3),
                (SYSTEMD_UNNAMED_FD.to_string(), 4),
                (SYSTEMD_UNNAMED_FD.to_string(), 5),
            ]
        );
    }

    #[test]
    fn parse_zero_or_missing_fds() {
        assert!(
            parse_systemd_env(Some("4242"), Some("0"), None, PID)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_systemd_env(Some("4242"), None, None, PID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn pool_take_is_once() {
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let pool = FdInheritanceConfig::from_fds([("a".into(), a.into()), ("b".into(), b.into())]);
        let clone = pool.clone();
        assert!(pool.get_fd("a").is_some());
        assert!(pool.take("a").is_some());
        assert!(clone.take("a").is_none());
        // Only "b" remains, so the sole-descriptor fallback applies.
        assert!(pool.take_named_or_sole("zzz").is_some());
        assert!(!pool.has_inherited_fds());
    }

    #[test]
    fn validation_detects_type_family_and_listening() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        validation::validate_socket_type(&tcp, libc::SOCK_STREAM).unwrap();
        validation::validate_socket_family(&tcp, libc::AF_INET).unwrap();
        validation::validate_listening(&tcp).unwrap();
        assert!(validation::validate_socket_type(&tcp, libc::SOCK_DGRAM).is_err());
        assert!(validation::validate_socket_family(&tcp, libc::AF_UNIX).is_err());

        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        validation::validate_socket_type(&udp, libc::SOCK_DGRAM).unwrap();

        let file = tempfile::tempfile().unwrap();
        assert!(validation::validate_socket_type(&file, libc::SOCK_STREAM).is_err());
    }
}
