//! Using sockets inherited from a parent process (systemd socket activation, a
//! process manager, a launcher that hands sockets over between restarts)
//! instead of binding them.
//!
//! 1. The parent creates and binds the sockets (and calls `listen` for stream
//!    sockets).
//! 2. It starts the server, passing the descriptors.
//! 3. The server turns each descriptor into a Tokio socket, after checking its
//!    socket type, address family and, for stream servers, listening state
//!    ([`validation`]).
//!
//! Inheritance is opt-in: a config's `bind_strategy` must be a
//! [`BindStrategy::Inherit`] or [`BindStrategy::InheritOrBind`] (e.g. via a
//! config's `with_fd_inheritance`). The default strategy binds.
//!
//! # systemd socket activation
//!
//! Servers look up descriptors in the process-wide pool returned by
//! [`FdInheritanceConfig::from_systemd_env`], which reads `LISTEN_PID`,
//! `LISTEN_FDS` and `LISTEN_FDNAMES` (see `sd_listen_fds(3)`). With
//! [`BindStrategy::InheritOrBind`] and no explicit descriptor, a server takes
//! the descriptor named after its `service_name` (`FileDescriptorName=` in the
//! `.socket` unit). If none matches and exactly one descriptor was passed in
//! total, it takes that one whatever its name; otherwise it binds its fallback
//! target ([`FdInheritanceConfig::take_named_or_sole`]).
//!
//! ```no_run
//! use echosrv::{EchoServerTrait, TcpConfig, TcpEchoServer};
//!
//! # #[tokio::main]
//! # async fn main() -> echosrv::Result<()> {
//! // Use the socket systemd passed as `FileDescriptorName=web`
//! // (or the only socket passed); bind 0.0.0.0:8080 when not socket-activated.
//! let config = TcpConfig {
//!     bind_addr: "0.0.0.0:8080".parse().unwrap(),
//!     ..TcpConfig::default()
//! }
//! .with_fd_inheritance("web");
//! TcpEchoServer::new(config.into()).run().await
//! # }
//! ```
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

/// What to bind when a socket is not inherited.
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

/// How a server obtains its socket: bind a new one, use an inherited
/// descriptor, or try inheriting and fall back to binding.
///
/// See the [module docs](self) for the lookup rules.
#[derive(Debug, Clone)]
pub enum BindStrategy {
    /// Always bind a new socket to the target (what the default configs do).
    Bind(BindTarget),

    /// Always use the given inherited file descriptor.
    ///
    /// Fails if the descriptor was already consumed, is not a socket, or has the
    /// wrong type/family for the protocol.
    Inherit(InheritedFd),

    /// Try to inherit first, fall back to binding if no descriptor is available.
    ///
    /// The explicit `fd` is tried first; if it is `None` (or already consumed),
    /// a descriptor is taken from the [`FdInheritanceConfig`] (e.g. systemd's
    /// `LISTEN_FDNAMES`) using
    /// [`take_named_or_sole`](FdInheritanceConfig::take_named_or_sole): the
    /// one named after the server's service name, else the only descriptor
    /// passed. If nothing is found, the fallback target is bound.
    InheritOrBind {
        /// Explicit descriptor to use first. With `None` (or once it has been
        /// consumed), the descriptor pool is consulted by service name.
        fd: Option<InheritedFd>,
        /// Target to bind to if inheritance fails.
        ///
        /// `None` means "the server config's own address": network configs
        /// ([`StreamConfig`](crate::stream::StreamConfig),
        /// [`DatagramConfig`](crate::datagram::DatagramConfig) and the TCP,
        /// UDP and HTTP configs) resolve it to their `bind_addr` when the
        /// socket is bound, so changing `bind_addr` after
        /// `with_fd_inheritance` takes effect. Unix configs have no such
        /// address and need an explicit target.
        fallback_target: Option<BindTarget>,
    },
}

impl BindStrategy {
    /// The target this strategy would bind to (either the `Bind` target or the
    /// `InheritOrBind` fallback). Returns `None` for [`BindStrategy::Inherit`]
    /// and for an `InheritOrBind` whose fallback has not been resolved yet.
    pub fn bind_target(&self) -> Option<&BindTarget> {
        match self {
            BindStrategy::Bind(target) => Some(target),
            BindStrategy::InheritOrBind {
                fallback_target, ..
            } => fallback_target.as_ref(),
            BindStrategy::Inherit(_) => None,
        }
    }

    /// Fills in an unset `InheritOrBind` fallback target with `default`.
    /// Other strategies are returned unchanged.
    pub fn with_default_fallback(self, default: BindTarget) -> Self {
        match self {
            BindStrategy::InheritOrBind {
                fd,
                fallback_target: None,
            } => BindStrategy::InheritOrBind {
                fd,
                fallback_target: Some(default),
            },
            other => other,
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
    /// Number of descriptors the pool was created with (before any `take`).
    /// Used by [`take_named_or_sole`](Self::take_named_or_sole) so that the
    /// "sole descriptor" fallback only applies when exactly one descriptor
    /// was ever passed, not when one happens to be left over.
    initial_count: usize,
}

/// systemd passes FDs starting from 3 (after stdin=0, stdout=1, stderr=2)
const SD_LISTEN_FDS_START: RawFd = 3;

/// Name systemd uses for descriptors without an explicit
/// `FileDescriptorName=`.
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
        let pool: Vec<NamedFd> = fds
            .into_iter()
            .map(|(name, fd)| NamedFd { name, fd })
            .collect();
        Self {
            initial_count: pool.len(),
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

        // SAFETY: systemd hands these descriptors to this process (LISTEN_PID
        // matched), and this function runs at most once per process (guarded
        // by `SYSTEMD_POOL`), so no other `OwnedFd` for them is ever created by
        // this crate.
        unsafe { Self::from_raw_named_fds(parsed) }
    }

    /// Builds a pool from raw descriptors, setting `FD_CLOEXEC` on each and
    /// skipping descriptors that are not open.
    ///
    /// # Safety
    ///
    /// Every *open* descriptor in `fds` must not be owned by anything else in
    /// the process: ownership is transferred to the pool.
    ///
    /// The pool's initial count is the number of descriptors *passed*, including
    /// skipped ones, so a parent that passed two descriptors (one of them
    /// invalid) never triggers the sole-descriptor fallback.
    unsafe fn from_raw_named_fds(fds: Vec<(String, RawFd)>) -> Self {
        let initial_count = fds.len();
        let mut pool = Vec::with_capacity(fds.len());
        for (name, raw) in fds {
            // Mark close-on-exec; this also checks that the descriptor is open.
            if let Err(e) = set_cloexec(raw) {
                tracing::warn!(fd = raw, name = %name, error = %e, "Skipping invalid inherited file descriptor");
                continue;
            }
            // SAFETY: the descriptor is open (fcntl succeeded above) and the
            // caller guarantees nothing else owns it.
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            pool.push(NamedFd { name, fd });
        }

        Self {
            pool: Arc::new(Mutex::new(pool)),
            initial_count,
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
    /// was *created* with exactly one descriptor that is still available, takes
    /// that one regardless of its name.
    ///
    /// This mirrors the common systemd setup where a single `.socket` unit
    /// passes one descriptor named after the unit (or `"unknown"`). The
    /// fallback is based on the initial descriptor count rather than on what is
    /// left, so in a process that was passed several descriptors a server can
    /// never pick up a leftover descriptor meant for a different service.
    ///
    /// # Examples
    ///
    /// ```
    /// use echosrv::network::FdInheritanceConfig;
    /// use std::os::fd::OwnedFd;
    ///
    /// let fd = |_| OwnedFd::from(std::net::TcpListener::bind("127.0.0.1:0").unwrap());
    ///
    /// // A single descriptor is used whatever its name.
    /// let single = FdInheritanceConfig::from_fds([("unknown".to_string(), fd(()))]);
    /// assert!(single.take_named_or_sole("web").is_some());
    ///
    /// // With several descriptors only exact names match, even once only one is left.
    /// let multi = FdInheritanceConfig::from_fds([
    ///     ("web".to_string(), fd(())),
    ///     ("api".to_string(), fd(())),
    /// ]);
    /// assert!(multi.take_named_or_sole("web").is_some());
    /// assert!(multi.take_named_or_sole("other").is_none());
    /// assert!(multi.take_named_or_sole("api").is_some());
    /// ```
    pub fn take_named_or_sole(&self, service_name: &str) -> Option<OwnedFd> {
        let mut pool = self.lock();
        if let Some(index) = pool.iter().position(|entry| entry.name == service_name) {
            return Some(pool.remove(index).fd);
        }
        if self.initial_count == 1 && pool.len() == 1 {
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
    use std::os::fd::IntoRawFd;
    use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};

    const PID: u32 = 4242;

    fn names(parsed: &[(String, RawFd)]) -> Vec<&str> {
        parsed.iter().map(|(name, _)| name.as_str()).collect()
    }

    fn tcp_fd() -> OwnedFd {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().into()
    }

    // --- parse_systemd_env -------------------------------------------------
    //
    // `from_systemd_env` reads the real process environment into a
    // process-wide `OnceLock`, and every server in this test binary calls it.
    // Setting `LISTEN_PID`/`LISTEN_FDS` to this process would make that pool
    // adopt fds 3.. (which belong to the test harness) and close them, so the
    // environment is deliberately never mutated here (it would also need
    // edition-2024 `unsafe { std::env::set_var }` while other test threads
    // run). The parsing rules are covered through the pure
    // `parse_systemd_env`, and the descriptor adoption step through
    // `from_raw_named_fds`.

    #[test]
    fn parse_requires_listen_pid() {
        assert!(
            parse_systemd_env(None, Some("2"), Some("a:b"), PID)
                .unwrap()
                .is_empty()
        );
        // Without LISTEN_PID even garbage elsewhere is ignored.
        assert!(
            parse_systemd_env(None, Some("garbage"), None, PID)
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
        // The PID check happens before LISTEN_FDS is parsed.
        assert!(
            parse_systemd_env(Some("1"), Some("garbage"), None, PID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parse_matching_pid_tolerates_whitespace() {
        let parsed = parse_systemd_env(Some(" 4242\n"), Some(" 1 "), None, PID).unwrap();
        assert_eq!(parsed, vec![(SYSTEMD_UNNAMED_FD.to_string(), 3)]);
    }

    #[test]
    fn parse_rejects_garbage() {
        for pid in ["abc", "", "-1", "4242x", "99999999999"] {
            let err = parse_systemd_env(Some(pid), Some("1"), None, PID).unwrap_err();
            assert!(err.contains("LISTEN_PID"), "{pid:?}: {err}");
        }
        for fds in ["x", "", "-1", "1.5", "65536"] {
            let err = parse_systemd_env(Some("4242"), Some(fds), None, PID).unwrap_err();
            assert!(err.contains("LISTEN_FDS"), "{fds:?}: {err}");
        }
    }

    #[test]
    fn parse_zero_or_missing_fds() {
        assert!(
            parse_systemd_env(Some("4242"), Some("0"), Some("a:b"), PID)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_systemd_env(Some("4242"), None, Some("a"), PID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parse_numbers_fds_consecutively_from_three() {
        let parsed = parse_systemd_env(Some("4242"), Some("4"), None, PID).unwrap();
        let fds: Vec<RawFd> = parsed.iter().map(|(_, fd)| *fd).collect();
        assert_eq!(fds, vec![3, 4, 5, 6]);
        assert!(names(&parsed).iter().all(|n| *n == SYSTEMD_UNNAMED_FD));
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
    fn parse_fewer_names_than_fds_defaults_the_rest() {
        let parsed = parse_systemd_env(Some("4242"), Some("3"), Some("http"), PID).unwrap();
        assert_eq!(
            names(&parsed),
            vec!["http", SYSTEMD_UNNAMED_FD, SYSTEMD_UNNAMED_FD]
        );
    }

    #[test]
    fn parse_more_names_than_fds_ignores_extras() {
        let parsed = parse_systemd_env(Some("4242"), Some("2"), Some("a:b:c:d"), PID).unwrap();
        assert_eq!(parsed, vec![("a".to_string(), 3), ("b".to_string(), 4)]);
    }

    #[test]
    fn parse_empty_names_and_empty_segments_default() {
        let parsed = parse_systemd_env(Some("4242"), Some("2"), Some(""), PID).unwrap();
        assert_eq!(names(&parsed), vec![SYSTEMD_UNNAMED_FD, SYSTEMD_UNNAMED_FD]);

        let parsed = parse_systemd_env(Some("4242"), Some("3"), Some(":mid:"), PID).unwrap();
        assert_eq!(
            names(&parsed),
            vec![SYSTEMD_UNNAMED_FD, "mid", SYSTEMD_UNNAMED_FD]
        );
    }

    #[test]
    fn parse_keeps_duplicate_names() {
        let parsed = parse_systemd_env(Some("4242"), Some("2"), Some("web:web"), PID).unwrap();
        assert_eq!(parsed, vec![("web".to_string(), 3), ("web".to_string(), 4)]);
    }

    // --- from_systemd_env / pool adoption ----------------------------------

    #[test]
    fn systemd_pool_is_process_wide_singleton() {
        let a = FdInheritanceConfig::from_systemd_env().unwrap();
        let b = FdInheritanceConfig::from_systemd_env().unwrap();
        assert!(Arc::ptr_eq(&a.pool, &b.pool));
    }

    #[test]
    fn raw_pool_adopts_open_fds_sets_cloexec_and_skips_closed() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let raw = listener.into_raw_fd();
        // Clear FD_CLOEXEC so we can observe the pool setting it.
        // SAFETY: plain fcntl on a descriptor we own.
        assert_eq!(unsafe { libc::fcntl(raw, libc::F_SETFD, 0) }, 0);

        // A descriptor number far above anything the test process opens.
        let closed: RawFd = 1_000_000;
        // SAFETY: `raw` is open and owned by nobody else (into_raw_fd released
        // it); `closed` is not open, so it is skipped without being adopted.
        let pool = unsafe {
            FdInheritanceConfig::from_raw_named_fds(vec![
                ("bad".to_string(), closed),
                ("tcp".to_string(), raw),
            ])
        };
        assert_eq!(pool.inherited_service_names(), vec!["tcp".to_string()]);
        assert_eq!(pool.get_fd("tcp"), Some(raw));
        // SAFETY: as above.
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0, "FD_CLOEXEC not set");

        let fd = pool.take("tcp").unwrap();
        let listener = std::net::TcpListener::from(fd);
        assert_eq!(listener.local_addr().unwrap(), addr);
    }

    #[test]
    fn set_cloexec_rejects_closed_fd() {
        let err = set_cloexec(1_000_000).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EBADF));
    }

    // --- FdInheritanceConfig pool ------------------------------------------

    #[test]
    fn empty_pool_has_nothing() {
        let pool = FdInheritanceConfig::empty();
        assert!(!pool.has_inherited_fds());
        assert!(pool.take("x").is_none());
        assert!(pool.take_named_or_sole("x").is_none());
        assert!(pool.get_fd("x").is_none());
        assert!(pool.inherited_service_names().is_empty());
        assert_eq!(format!("{pool:?}"), "{}");
    }

    #[test]
    fn pool_take_is_once() {
        let pool = FdInheritanceConfig::from_fds([("a".into(), tcp_fd()), ("b".into(), tcp_fd())]);
        let clone = pool.clone();
        let raw_a = pool.get_fd("a").unwrap();
        // get_fd does not consume.
        assert_eq!(pool.get_fd("a"), Some(raw_a));
        assert_eq!(pool.take("a").unwrap().as_raw_fd(), raw_a);
        // Clones share the pool.
        assert!(clone.take("a").is_none());
        assert!(clone.get_fd("a").is_none());
        assert_eq!(clone.inherited_service_names(), vec!["b".to_string()]);
        assert!(clone.has_inherited_fds());
    }

    #[test]
    fn pool_duplicate_names_are_taken_in_order() {
        let first = tcp_fd();
        let second = tcp_fd();
        let (raw1, raw2) = (first.as_raw_fd(), second.as_raw_fd());
        let pool = FdInheritanceConfig::from_fds([
            ("web".to_string(), first),
            ("web".to_string(), second),
        ]);
        assert_eq!(
            pool.inherited_service_names(),
            vec!["web".to_string(), "web".to_string()]
        );
        assert_eq!(pool.get_fd("web"), Some(raw1));
        assert_eq!(pool.take("web").unwrap().as_raw_fd(), raw1);
        assert_eq!(pool.take("web").unwrap().as_raw_fd(), raw2);
        assert!(pool.take("web").is_none());
    }

    #[test]
    fn take_named_or_sole_prefers_name() {
        // Named match among several.
        let pool = FdInheritanceConfig::from_fds([("a".into(), tcp_fd()), ("b".into(), tcp_fd())]);
        let raw_b = pool.get_fd("b").unwrap();
        assert_eq!(pool.take_named_or_sole("b").unwrap().as_raw_fd(), raw_b);
        let raw_a = pool.get_fd("a").unwrap();
        assert_eq!(pool.take_named_or_sole("a").unwrap().as_raw_fd(), raw_a);
        assert!(!pool.has_inherited_fds());
    }

    #[test]
    fn take_named_or_sole_uses_sole_fd_of_single_fd_pool() {
        let fd = tcp_fd();
        let raw = fd.as_raw_fd();
        let pool = FdInheritanceConfig::from_fds([(SYSTEMD_UNNAMED_FD.to_string(), fd)]);
        assert_eq!(pool.take_named_or_sole("zzz").unwrap().as_raw_fd(), raw);
        assert!(!pool.has_inherited_fds());
        assert!(pool.take_named_or_sole("zzz").is_none());
    }

    /// In a multi-server process the sole-fd fallback must not hand a
    /// leftover descriptor (meant for another service) to a later server.
    #[test]
    fn take_named_or_sole_ignores_leftover_of_multi_fd_pool() {
        let pool = FdInheritanceConfig::from_fds([("a".into(), tcp_fd()), ("b".into(), tcp_fd())]);
        let raw_b = pool.get_fd("b").unwrap();
        assert!(pool.take_named_or_sole("a").is_some());
        // Only "b" is left, but the pool started with two descriptors.
        assert!(pool.take_named_or_sole("zzz").is_none());
        assert_eq!(pool.get_fd("b"), Some(raw_b));
        // Clones share the initial count.
        assert!(pool.clone().take_named_or_sole("zzz").is_none());
        assert_eq!(pool.take_named_or_sole("b").unwrap().as_raw_fd(), raw_b);
    }

    #[test]
    fn raw_pool_counts_skipped_fds_for_sole_rule() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let raw = listener.into_raw_fd();
        // SAFETY: `raw` is open and released by its previous owner; 1_000_000
        // is not open and is skipped without being adopted.
        let pool = unsafe {
            FdInheritanceConfig::from_raw_named_fds(vec![
                ("bad".to_string(), 1_000_000),
                ("tcp".to_string(), raw),
            ])
        };
        assert!(pool.take_named_or_sole("other").is_none());
        assert_eq!(pool.take_named_or_sole("tcp").unwrap().as_raw_fd(), raw);
    }

    #[test]
    fn take_named_or_sole_is_none_when_ambiguous() {
        let pool = FdInheritanceConfig::from_fds([("a".into(), tcp_fd()), ("b".into(), tcp_fd())]);
        assert!(pool.take_named_or_sole("zzz").is_none());
        // Nothing was taken.
        assert_eq!(pool.inherited_service_names().len(), 2);
    }

    #[test]
    fn pool_debug_lists_names_and_fds() {
        let fd = tcp_fd();
        let raw = fd.as_raw_fd();
        let pool = FdInheritanceConfig::from_fds([("svc".to_string(), fd)]);
        assert_eq!(format!("{pool:?}"), format!("{{\"svc\": {raw}}}"));
    }

    // --- InheritedFd / BindStrategy ----------------------------------------

    #[test]
    fn inherited_fd_take_once_across_clones() {
        let owned = tcp_fd();
        let raw = owned.as_raw_fd();
        let fd = InheritedFd::from(owned);
        let clone = fd.clone();
        assert_eq!(fd.raw_fd(), raw);
        assert!(!clone.is_consumed());
        assert!(format!("{fd:?}").contains("consumed: false"));

        assert_eq!(clone.take().unwrap().as_raw_fd(), raw);
        assert!(fd.is_consumed());
        assert!(fd.take().is_none());
        // The raw number stays available for diagnostics.
        assert_eq!(fd.raw_fd(), raw);
        assert!(format!("{fd:?}").contains("consumed: true"));
    }

    #[test]
    fn inherited_fd_from_raw_fd_takes_ownership() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let raw = listener.into_raw_fd();
        // SAFETY: `raw` is open and its previous owner released it.
        let fd = unsafe { InheritedFd::from_raw_fd(raw) };
        let listener = std::net::TcpListener::from(fd.take().unwrap());
        assert_eq!(listener.local_addr().unwrap(), addr);
    }

    #[test]
    fn bind_strategy_bind_target() {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let bind = BindStrategy::Bind(BindTarget::Network(addr));
        assert!(matches!(bind.bind_target(), Some(BindTarget::Network(a)) if *a == addr));

        let fallback = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix("/x.sock".into())),
        };
        assert!(
            matches!(fallback.bind_target(), Some(BindTarget::Unix(p)) if p.as_os_str() == "/x.sock")
        );

        let unresolved = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: None,
        };
        assert!(unresolved.bind_target().is_none());

        let inherit = BindStrategy::Inherit(tcp_fd().into());
        assert!(inherit.bind_target().is_none());
    }

    #[test]
    fn with_default_fallback_fills_only_unset_fallback() {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let default = || BindTarget::Network(addr);

        let unresolved = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: None,
        };
        assert!(matches!(
            unresolved.with_default_fallback(default()).bind_target(),
            Some(BindTarget::Network(a)) if *a == addr
        ));

        // An explicit fallback is kept.
        let explicit = BindStrategy::InheritOrBind {
            fd: None,
            fallback_target: Some(BindTarget::Unix("/x.sock".into())),
        };
        assert!(matches!(
            explicit.with_default_fallback(default()).bind_target(),
            Some(BindTarget::Unix(_))
        ));

        // The explicit fd survives and other strategies are untouched.
        let fd = InheritedFd::from(tcp_fd());
        let with_fd = BindStrategy::InheritOrBind {
            fd: Some(fd.clone()),
            fallback_target: None,
        }
        .with_default_fallback(default());
        match with_fd {
            BindStrategy::InheritOrBind {
                fd: Some(inner), ..
            } => {
                assert!(inner.take().is_some());
                assert!(fd.is_consumed());
            }
            other => panic!("unexpected {other:?}"),
        }
        let bind =
            BindStrategy::Bind(BindTarget::Unix("/y.sock".into())).with_default_fallback(default());
        assert!(matches!(bind, BindStrategy::Bind(BindTarget::Unix(_))));
        let inherit = BindStrategy::Inherit(tcp_fd().into()).with_default_fallback(default());
        assert!(inherit.bind_target().is_none());
    }

    // --- validation ----------------------------------------------------------

    use validation::{validate_listening, validate_socket_family, validate_socket_type};

    #[track_caller]
    fn assert_fd_err(result: Result<()>, needle: &str) {
        match result {
            Err(EchoError::FdInheritance(msg)) => {
                assert!(msg.contains(needle), "{msg:?} does not mention {needle:?}")
            }
            other => panic!("expected FdInheritance error, got {other:?}"),
        }
    }

    /// Whether the platform reports SO_ACCEPTCONN (some platforms do not, in
    /// which case `validate_listening` skips the check).
    fn acceptconn_supported() -> bool {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: valid out-pointers for an int-valued option.
        let rc = unsafe {
            libc::getsockopt(
                listener.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ACCEPTCONN,
                &mut value as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        rc == 0
    }

    #[test]
    fn validate_tcp_listener() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        validate_socket_type(&tcp, libc::SOCK_STREAM).unwrap();
        validate_socket_family(&tcp, libc::AF_INET).unwrap();
        validate_listening(&tcp).unwrap();
        assert_fd_err(validate_socket_type(&tcp, libc::SOCK_DGRAM), "SOCK_DGRAM");
        assert_fd_err(
            validate_socket_family(&tcp, libc::AF_UNIX),
            "AF_INET (IPv4)",
        );
        assert_fd_err(
            validate_socket_family(&tcp, libc::AF_INET6),
            "expected AF_INET6",
        );
    }

    #[test]
    fn validate_tcp_ipv6_listener() {
        let Ok(tcp) = std::net::TcpListener::bind("[::1]:0") else {
            eprintln!("IPv6 loopback unavailable; skipping");
            return;
        };
        validate_socket_family(&tcp, libc::AF_INET6).unwrap();
        assert_fd_err(
            validate_socket_family(&tcp, libc::AF_INET),
            "AF_INET6 (IPv6)",
        );
    }

    #[test]
    fn validate_connected_tcp_stream_is_not_listening() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        for socket in [stream.as_fd(), accepted.as_fd()] {
            validate_socket_type(socket, libc::SOCK_STREAM).unwrap();
            validate_socket_family(socket, libc::AF_INET).unwrap();
            if acceptconn_supported() {
                assert_fd_err(validate_listening(socket), "not a listening socket");
            } else {
                validate_listening(socket).unwrap();
            }
        }
    }

    #[test]
    fn validate_udp_socket() {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        validate_socket_type(&udp, libc::SOCK_DGRAM).unwrap();
        validate_socket_family(&udp, libc::AF_INET).unwrap();
        assert_fd_err(validate_socket_type(&udp, libc::SOCK_STREAM), "SOCK_STREAM");
        assert_fd_err(validate_socket_family(&udp, libc::AF_UNIX), "AF_UNIX");
        if acceptconn_supported() {
            assert_fd_err(validate_listening(&udp), "not a listening socket");
        }
    }

    #[test]
    fn validate_unix_stream_listener_and_pair() {
        let dir = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(dir.path().join("s.sock")).unwrap();
        validate_socket_type(&listener, libc::SOCK_STREAM).unwrap();
        validate_socket_family(&listener, libc::AF_UNIX).unwrap();
        validate_listening(&listener).unwrap();
        assert_fd_err(validate_socket_family(&listener, libc::AF_INET), "AF_UNIX");

        let (a, _b) = UnixStream::pair().unwrap();
        validate_socket_type(&a, libc::SOCK_STREAM).unwrap();
        validate_socket_family(&a, libc::AF_UNIX).unwrap();
        if acceptconn_supported() {
            assert_fd_err(validate_listening(&a), "not a listening socket");
        }
    }

    #[test]
    fn validate_unix_datagram() {
        let dir = tempfile::tempdir().unwrap();
        let socket = UnixDatagram::bind(dir.path().join("d.sock")).unwrap();
        validate_socket_type(&socket, libc::SOCK_DGRAM).unwrap();
        validate_socket_family(&socket, libc::AF_UNIX).unwrap();
        assert_fd_err(
            validate_socket_type(&socket, libc::SOCK_STREAM),
            "SOCK_STREAM",
        );

        // Unbound sockets still report their family.
        let unbound = UnixDatagram::unbound().unwrap();
        validate_socket_family(&unbound, libc::AF_UNIX).unwrap();
    }

    #[test]
    fn validate_regular_file_is_rejected() {
        let file = tempfile::tempfile().unwrap();
        assert_fd_err(
            validate_socket_type(&file, libc::SOCK_STREAM),
            "Failed to get socket type",
        );
        assert_fd_err(
            validate_socket_family(&file, libc::AF_INET),
            "Failed to get socket address",
        );
        assert_fd_err(validate_listening(&file), "SO_ACCEPTCONN");
    }

    #[test]
    fn validate_unknown_expected_values_are_named() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert_fd_err(
            validate_socket_type(&tcp, libc::SOCK_RAW),
            "unknown socket type",
        );
        assert_fd_err(
            validate_socket_family(&tcp, libc::AF_UNSPEC),
            "unknown address family",
        );
    }
}
