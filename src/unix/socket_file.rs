//! Ownership of Unix socket files: stale-file recovery at bind time and
//! removal on drop (only for files this process created).

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

/// A socket file created by this process; removed when dropped.
///
/// The file's device/inode are recorded at bind time and checked again before
/// removal, so a file that was replaced by someone else is left alone.
#[derive(Debug)]
pub(crate) struct SocketFile {
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl SocketFile {
    /// Records ownership of the socket file at `path` (call right after bind).
    pub(crate) fn record(path: &Path) -> io::Result<Self> {
        let meta = std::fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            dev: meta.dev(),
            ino: meta.ino(),
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

/// Which kind of socket a path is expected to host (used to probe liveness).
#[derive(Debug, Clone, Copy)]
pub(crate) enum SocketKind {
    Stream,
    Datagram,
}

/// Binds a Unix socket at `path`, recovering from a stale socket file.
///
/// Parent directories are created if missing. If the path is in use, it is
/// removed and the bind retried only when it is a socket and connecting to it
/// is refused (nobody is listening). A live socket or a non-socket file yields
/// an `AddrInUse` error.
pub(crate) fn bind_with_stale_recovery<T>(
    path: &Path,
    kind: SocketKind,
    bind: impl Fn(&Path) -> io::Result<T>,
) -> io::Result<T> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    match bind(path) {
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            let meta = std::fs::symlink_metadata(path)?;
            if !meta.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("{} exists and is not a socket", path.display()),
                ));
            }
            if !is_stale(path, kind) {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another process is listening on {}", path.display()),
                ));
            }
            tracing::info!(path = %path.display(), "Removing stale socket file");
            std::fs::remove_file(path)?;
            bind(path)
        }
        other => other,
    }
}

/// A socket file is stale if connecting to it is refused.
fn is_stale(path: &Path, kind: SocketKind) -> bool {
    let probe = match kind {
        SocketKind::Stream => std::os::unix::net::UnixStream::connect(path).map(drop),
        SocketKind::Datagram => {
            std::os::unix::net::UnixDatagram::unbound().and_then(|socket| socket.connect(path))
        }
    };
    matches!(probe, Err(e) if e.kind() == io::ErrorKind::ConnectionRefused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_stream_socket_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.sock");
        // Create a socket file and close the listener without unlinking it.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let listener = bind_with_stale_recovery(&path, SocketKind::Stream, |p| {
            std::os::unix::net::UnixListener::bind(p)
        })
        .unwrap();
        std::os::unix::net::UnixStream::connect(&path).unwrap();
        drop(listener);
    }

    #[test]
    fn live_socket_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.sock");
        let _live = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let err = bind_with_stale_recovery(&path, SocketKind::Stream, |p| {
            std::os::unix::net::UnixListener::bind(p)
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn regular_file_is_not_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.sock");
        std::fs::write(&path, b"data").unwrap();
        let err = bind_with_stale_recovery(&path, SocketKind::Datagram, |p| {
            std::os::unix::net::UnixDatagram::bind(p)
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
    }

    #[test]
    fn socket_file_removed_on_drop_only_if_same_inode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let file = SocketFile::record(&path).unwrap();
        drop(listener);
        drop(file);
        assert!(!path.exists());

        // Replaced file: not removed.
        let first = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let file = SocketFile::record(&path).unwrap();
        drop(first);
        std::fs::remove_file(&path).unwrap();
        let _second = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(file);
        assert!(path.exists());
    }

    #[test]
    fn missing_parent_directories_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.sock");
        let _listener = bind_with_stale_recovery(&path, SocketKind::Stream, |p| {
            std::os::unix::net::UnixListener::bind(p)
        })
        .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn stale_datagram_socket_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale-dgram.sock");
        drop(std::os::unix::net::UnixDatagram::bind(&path).unwrap());
        assert!(path.exists());

        let socket = bind_with_stale_recovery(&path, SocketKind::Datagram, |p| {
            std::os::unix::net::UnixDatagram::bind(p)
        })
        .unwrap();
        let sender = std::os::unix::net::UnixDatagram::unbound().unwrap();
        sender.send_to(b"hi", &path).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(socket.recv(&mut buf).unwrap(), 2);
    }

    #[test]
    fn live_datagram_socket_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live-dgram.sock");
        let _live = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        let err = bind_with_stale_recovery(&path, SocketKind::Datagram, |p| {
            std::os::unix::net::UnixDatagram::bind(p)
        })
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
    }

    #[test]
    fn record_missing_path_fails_and_drop_tolerates_removed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.sock");
        assert!(SocketFile::record(&path).is_err());

        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let file = SocketFile::record(&path).unwrap();
        assert_eq!(file.path(), path.as_path());
        drop(listener);
        std::fs::remove_file(&path).unwrap();
        drop(file); // must not panic
        assert!(!path.exists());
    }
}
