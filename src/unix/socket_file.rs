//! Ownership of Unix socket files: stale-file recovery at bind time and
//! removal on drop (only for files this process created).
//!
//! # Deciding whether a socket file is stale
//!
//! A path that fails to bind with `EADDRINUSE` is replaced only if nobody is
//! using it. Connecting to it is the only portable way to ask, and the answer
//! differs between kernels:
//!
//! - **Linux.** A stream connect to a path with no socket behind it, or to a
//!   socket that is not listening, fails with `ECONNREFUSED`. A connect to a
//!   live listener whose accept backlog is full *blocks*, or fails with
//!   `EAGAIN` on a non-blocking socket. So a non-blocking probe is
//!   unambiguous: `ECONNREFUSED` means stale, `EAGAIN` means live.
//! - **macOS and the BSDs.** `unp_connect` fails with `ECONNREFUSED` both when
//!   nobody listens and when the listener's backlog is full (`sonewconn`
//!   returns no socket). The connect never blocks or returns `EAGAIN`, so a
//!   probe alone cannot tell an overloaded server from a dead one.
//! - **Datagram sockets** have no backlog: connecting to a live one succeeds
//!   and to a dead one fails with `ECONNREFUSED` on every platform.
//!
//! To close the macOS gap, a stream server also holds an advisory `flock` on
//! a sidecar file, `<path>.lock`, for as long as it owns the socket file. A
//! second server that finds the lock held reports `AddrInUse` without probing
//! at all; only while it holds the lock does it probe and, if the file is
//! stale, remove it. The kernel drops the lock when the holder dies, so a
//! crash never leaves the path locked. The lock also serializes stale
//! recovery, so two servers starting at once cannot both "recover" the path
//! and unlink each other's fresh socket. A listener that does not take the
//! lock (another program, or a socket inherited from systemd) is still judged
//! by the probe alone, so on macOS an overloaded foreign listener can be
//! mistaken for a stale file; the probe is all that platform offers for it.
//!
//! The probe uses a non-blocking socket, so binding never waits on another
//! process's backlog while running on an async runtime thread.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// A socket file created by this process; removed when dropped.
///
/// The file's device/inode are recorded at bind time and checked again before
/// removal, so a file that was replaced by someone else is left alone. For
/// stream sockets it also holds the `<path>.lock` advisory lock (see the
/// module docs), which is released only after the socket file is removed.
#[derive(Debug)]
pub(crate) struct SocketFile {
    path: PathBuf,
    dev: u64,
    ino: u64,
    // Fields drop after `Drop::drop` has removed the socket file.
    _lock: Option<LockFile>,
}

impl SocketFile {
    /// Records ownership of the socket file at `path` (call right after bind).
    fn record(path: &Path, lock: Option<LockFile>) -> io::Result<Self> {
        let meta = std::fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            dev: meta.dev(),
            ino: meta.ino(),
            _lock: lock,
        })
    }

    /// Path of the owned socket file.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        match std::fs::symlink_metadata(&self.path) {
            Ok(meta) if meta.dev() == self.dev && meta.ino() == self.ino => {
                if let Err(e) = std::fs::remove_file(&self.path) {
                    tracing::warn!(path = %self.path.display(), error = %e, "Failed to remove socket file");
                }
            }
            _ => {}
        }
    }
}

/// An exclusive advisory `flock` on `<socket path>.lock`.
///
/// The lock file is removed on drop while the lock is still held. Acquiring
/// re-checks that the path still names the locked file, so a holder that
/// unlinks it cannot hand the same lock to two successors.
#[derive(Debug)]
struct LockFile {
    path: PathBuf,
    file: File,
}

impl LockFile {
    /// Path of the lock file guarding the socket at `socket_path`.
    fn path_for(socket_path: &Path) -> PathBuf {
        let mut path = socket_path.as_os_str().to_owned();
        path.push(".lock");
        PathBuf::from(path)
    }

    /// Takes the lock without waiting. `Ok(None)` means another process holds
    /// it; an error means locking is not possible here (e.g. permissions, or a
    /// file system without `flock`).
    fn try_acquire(socket_path: &Path) -> io::Result<Option<Self>> {
        let path = Self::path_for(socket_path);
        loop {
            let file = match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o644)
                .open(&path)
            {
                // A lock file left by another user: `flock` works read-only.
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => File::open(&path)?,
                other => other?,
            };
            // SAFETY: flock only operates on the descriptor, which `file` keeps
            // open for the duration of the call; no memory is accessed.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let err = io::Error::last_os_error();
                match err.kind() {
                    io::ErrorKind::WouldBlock => return Ok(None),
                    io::ErrorKind::Interrupted => continue,
                    _ => return Err(err),
                }
            }
            // The previous holder may have unlinked the file between our open
            // and flock; then we locked an orphan and must start over.
            let held = file.metadata()?;
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.dev() == held.dev() && meta.ino() == held.ino() => {
                    return Ok(Some(Self { path, file }));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        // Unlink while still locked; the lock is released when `file` closes.
        match (self.file.metadata(), std::fs::symlink_metadata(&self.path)) {
            (Ok(held), Ok(meta)) if meta.dev() == held.dev() && meta.ino() == held.ino() => {
                if let Err(e) = std::fs::remove_file(&self.path) {
                    tracing::warn!(path = %self.path.display(), error = %e, "Failed to remove lock file");
                }
            }
            _ => {}
        }
    }
}

/// Which kind of socket a path is expected to host (used to probe liveness).
#[derive(Debug, Clone, Copy)]
pub(crate) enum SocketKind {
    Stream,
    Datagram,
}

/// Binds a Unix socket at `path`, recovering from a stale socket file, and
/// returns the socket together with ownership of the file it created.
///
/// Parent directories are created if missing. Stream sockets first take the
/// `<path>.lock` advisory lock; if another server holds it, the path is in
/// use. If the bind finds the path taken, it is removed and the bind retried
/// only when it is a socket and a non-blocking probe finds nobody behind it
/// (see the module docs). A live socket or a non-socket file yields an
/// `AddrInUse` error.
pub(crate) fn bind_with_stale_recovery<T>(
    path: &Path,
    kind: SocketKind,
    bind: impl Fn(&Path) -> io::Result<T>,
) -> io::Result<(T, SocketFile)> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let in_use = || {
        io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another process is listening on {}", path.display()),
        )
    };

    let lock = match kind {
        SocketKind::Datagram => None,
        SocketKind::Stream => match LockFile::try_acquire(path) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => return Err(in_use()),
            Err(e) => {
                // Degrade to the probe alone rather than refusing to start.
                tracing::warn!(
                    path = %LockFile::path_for(path).display(),
                    error = %e,
                    "Cannot lock socket lock file; relying on the connect probe only"
                );
                None
            }
        },
    };

    let socket = match bind(path) {
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            let meta = std::fs::symlink_metadata(path)?;
            if !meta.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("{} exists and is not a socket", path.display()),
                ));
            }
            if probe(path, kind) == Probe::Live {
                return Err(in_use());
            }
            tracing::info!(path = %path.display(), "Removing stale socket file");
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            bind(path)?
        }
        other => other?,
    };
    let file = SocketFile::record(path, lock)?;
    Ok((socket, file))
}

/// Outcome of probing a socket file for a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Something is (or may be) using the socket; leave it alone.
    Live,
    /// Nobody is behind the socket file, or it vanished; safe to remove.
    Stale,
}

/// Probes `path` with a connect that never blocks.
///
/// Only a clear "no peer" answer is stale: `ECONNREFUSED`, or `ENOENT` if the
/// file vanished meanwhile. Success, a full backlog (`EAGAIN`, or
/// `EINPROGRESS` should a kernel report it) and any other error count as
/// live, so an unexpected answer never deletes a file.
fn probe(path: &Path, kind: SocketKind) -> Probe {
    let result = match kind {
        SocketKind::Stream => connect_stream_nonblocking(path).map(drop),
        // A datagram connect only records the peer address; it never waits.
        SocketKind::Datagram => {
            std::os::unix::net::UnixDatagram::unbound().and_then(|socket| socket.connect(path))
        }
    };
    match result {
        Err(e) if matches!(e.raw_os_error(), Some(libc::ECONNREFUSED | libc::ENOENT)) => {
            Probe::Stale
        }
        _ => Probe::Live,
    }
}

/// Connects a new non-blocking `SOCK_STREAM` socket to `path`.
///
/// `std::os::unix::net::UnixStream::connect` blocks on Linux while the
/// listener's backlog is full, so the socket is built by hand instead.
fn connect_stream_nonblocking(path: &Path) -> io::Result<OwnedFd> {
    // SAFETY: sockaddr_un is plain old data; all-zero is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    // Leave room for the NUL terminator.
    if bytes.is_empty() || bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, &src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = src as libc::c_char;
    }
    let len = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        // `len` is at most size_of::<sockaddr_un>() (106 on these platforms).
        addr.sun_len = len as u8;
    }

    let fd = new_nonblocking_stream_socket()?;
    // SAFETY: `addr` is a valid sockaddr_un initialized for `len` bytes, and
    // `fd` is an open socket owned by this function.
    let rc = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const addr).cast::<libc::sockaddr>(),
            len as libc::socklen_t,
        )
    };
    if rc == 0 {
        Ok(fd)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A new `AF_UNIX` stream socket with `O_NONBLOCK` and `FD_CLOEXEC` set.
fn new_nonblocking_stream_socket() -> io::Result<OwnedFd> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let ty = libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let ty = libc::SOCK_STREAM;

    // SAFETY: socket() takes no pointers; a non-negative result is a new
    // descriptor that we immediately take ownership of.
    let raw = unsafe { libc::socket(libc::AF_UNIX, ty, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a freshly created, open descriptor owned by nobody else.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let raw = fd.as_raw_fd();
        // SAFETY: fcntl with F_GETFD/F_SETFD/F_GETFL/F_SETFL only reads and
        // writes flags of the open descriptor `raw`; no memory is accessed.
        unsafe {
            let fd_flags = libc::fcntl(raw, libc::F_GETFD);
            if fd_flags < 0 || libc::fcntl(raw, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
            let fl_flags = libc::fcntl(raw, libc::F_GETFL);
            if fl_flags < 0 || libc::fcntl(raw, libc::F_SETFL, fl_flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};

    fn bind_stream(path: &Path) -> io::Result<(UnixListener, SocketFile)> {
        bind_with_stale_recovery(path, SocketKind::Stream, |p| UnixListener::bind(p))
    }

    /// Shrinks `listener`'s backlog to 1 and fills it with pending
    /// connections (bounded), returning the connections that keep it full.
    fn fill_backlog(listener: &UnixListener, path: &Path) -> Vec<OwnedFd> {
        // SAFETY: re-listening on an open listening socket only updates its
        // backlog; no memory is accessed.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let mut pending = Vec::new();
        for _ in 0..16 {
            match connect_stream_nonblocking(path) {
                Ok(fd) => pending.push(fd),
                Err(_) => return pending,
            }
        }
        panic!("backlog never filled after {} connections", pending.len());
    }

    #[test]
    fn probe_missing_file_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.sock");
        assert_eq!(probe(&path, SocketKind::Stream), Probe::Stale);
        assert_eq!(probe(&path, SocketKind::Datagram), Probe::Stale);
    }

    #[test]
    fn probe_closed_socket_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let stream = dir.path().join("stream.sock");
        let dgram = dir.path().join("dgram.sock");
        drop(UnixListener::bind(&stream).unwrap());
        drop(UnixDatagram::bind(&dgram).unwrap());
        assert!(stream.exists() && dgram.exists());
        assert_eq!(probe(&stream, SocketKind::Stream), Probe::Stale);
        assert_eq!(probe(&dgram, SocketKind::Datagram), Probe::Stale);
    }

    #[test]
    fn probe_live_socket_is_live() {
        let dir = tempfile::tempdir().unwrap();
        let stream = dir.path().join("stream.sock");
        let dgram = dir.path().join("dgram.sock");
        let _listener = UnixListener::bind(&stream).unwrap();
        let _socket = UnixDatagram::bind(&dgram).unwrap();
        assert_eq!(probe(&stream, SocketKind::Stream), Probe::Live);
        assert_eq!(probe(&dgram, SocketKind::Datagram), Probe::Live);
    }

    /// Linux reports a full backlog as `EAGAIN` to a non-blocking connect,
    /// which must count as live (a blocking connect would hang here).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn probe_full_backlog_is_live_on_linux() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let _pending = fill_backlog(&listener, &path);
        let err = connect_stream_nonblocking(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(probe(&path, SocketKind::Stream), Probe::Live);
    }

    /// On every platform (including macOS, where a full backlog looks like
    /// `ECONNREFUSED`), the lock keeps an overloaded server's socket safe.
    #[test]
    fn server_with_full_backlog_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("busy.sock");
        let (listener, _file) = bind_stream(&path).unwrap();
        let _pending = fill_backlog(&listener, &path);

        let err = bind_stream(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
        assert!(LockFile::path_for(&path).exists());
    }

    #[test]
    fn lock_file_is_held_while_bound_and_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.sock");
        let lock_path = LockFile::path_for(&path);
        let (listener, file) = bind_stream(&path).unwrap();
        assert!(LockFile::try_acquire(&path).unwrap().is_none());

        drop(listener);
        drop(file);
        assert!(!path.exists());
        assert!(!lock_path.exists());
        // Free again, and a fresh bind works.
        let (_listener, _file) = bind_stream(&path).unwrap();
    }

    #[test]
    fn leftover_unlocked_lock_file_does_not_block_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crashed.sock");
        // A crashed server leaves both files behind, but no lock.
        drop(UnixListener::bind(&path).unwrap());
        std::fs::write(LockFile::path_for(&path), b"").unwrap();

        let (_listener, _file) = bind_stream(&path).unwrap();
        UnixStream::connect(&path).unwrap();
    }

    #[test]
    fn stale_stream_socket_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.sock");
        // Create a socket file and close the listener without unlinking it.
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let (listener, _file) = bind_stream(&path).unwrap();
        UnixStream::connect(&path).unwrap();
        drop(listener);
    }

    #[test]
    fn live_socket_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.sock");
        // A listener that does not hold the lock: the probe must catch it.
        let _live = UnixListener::bind(&path).unwrap();
        let err = bind_stream(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
    }

    #[test]
    fn regular_file_is_not_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.sock");
        std::fs::write(&path, b"data").unwrap();
        let err = bind_with_stale_recovery(&path, SocketKind::Datagram, |p| UnixDatagram::bind(p))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
    }

    #[test]
    fn socket_file_removed_on_drop_only_if_same_inode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let file = SocketFile::record(&path, None).unwrap();
        drop(listener);
        drop(file);
        assert!(!path.exists());

        // Replaced file: not removed.
        let first = UnixListener::bind(&path).unwrap();
        let file = SocketFile::record(&path, None).unwrap();
        drop(first);
        std::fs::remove_file(&path).unwrap();
        let _second = UnixListener::bind(&path).unwrap();
        drop(file);
        assert!(path.exists());
    }

    #[test]
    fn missing_parent_directories_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.sock");
        let _bound = bind_stream(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn stale_datagram_socket_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-dgram.sock");
        drop(UnixDatagram::bind(&path).unwrap());
        assert!(path.exists());

        let (socket, _file) =
            bind_with_stale_recovery(&path, SocketKind::Datagram, |p| UnixDatagram::bind(p))
                .unwrap();
        let sender = UnixDatagram::unbound().unwrap();
        sender.send_to(b"hi", &path).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(socket.recv(&mut buf).unwrap(), 2);
        // Datagram sockets take no lock.
        assert!(!LockFile::path_for(&path).exists());
    }

    #[test]
    fn live_datagram_socket_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live-dgram.sock");
        let _live = UnixDatagram::bind(&path).unwrap();
        let err = bind_with_stale_recovery(&path, SocketKind::Datagram, |p| UnixDatagram::bind(p))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
    }

    #[test]
    fn other_bind_errors_are_returned_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.sock");
        let calls = std::cell::Cell::new(0);
        let err = bind_with_stale_recovery(&path, SocketKind::Stream, |_| {
            calls.set(calls.get() + 1);
            Err::<(), _>(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            calls.get(),
            1,
            "non-AddrInUse errors must not trigger a retry"
        );
        assert!(!LockFile::path_for(&path).exists(), "lock file left behind");
    }

    #[test]
    fn record_missing_path_fails_and_drop_tolerates_removed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.sock");
        assert!(SocketFile::record(&path, None).is_err());

        let listener = UnixListener::bind(&path).unwrap();
        let file = SocketFile::record(&path, None).unwrap();
        assert_eq!(file.path(), path.as_path());
        drop(listener);
        std::fs::remove_file(&path).unwrap();
        drop(file); // must not panic
        assert!(!path.exists());
    }
}
