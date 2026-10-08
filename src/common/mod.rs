//! Traits and types shared by every echo server and client.
//!
//! [`EchoServerTrait`] is implemented by all servers and [`EchoClient`] by all
//! clients, so code can be written once against either protocol family.
//! [`ServerStats`] holds the counters every server keeps.

pub(crate) mod lifecycle;
pub mod stats;
pub mod traits;

pub use stats::ServerStats;
pub use traits::{EchoClient, EchoServerTrait};

use crate::{EchoError, Result};
use std::borrow::Cow;
use std::time::Duration;

/// Longest payload prefix, in bytes, shown in `trace!` previews.
pub(crate) const PREVIEW_LEN: usize = 64;

/// A short, lossy UTF-8 preview of `payload` for trace logs: at most
/// [`PREVIEW_LEN`] bytes, so tracing a 64 KiB chunk stays cheap.
pub(crate) fn payload_preview(payload: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(&payload[..payload.len().min(PREVIEW_LEN)])
}

/// Rejects zero `read_timeout`/`write_timeout`: a zero timeout expires on
/// the first wait, so every connection (or send) would fail at once.
pub(crate) fn validate_timeouts(read_timeout: Duration, write_timeout: Duration) -> Result<()> {
    for (field, value) in [
        ("read_timeout", read_timeout),
        ("write_timeout", write_timeout),
    ] {
        if value.is_zero() {
            return Err(EchoError::Config(format!("{field} must be greater than 0")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_preview_is_capped() {
        assert_eq!(payload_preview(b"hello"), "hello");
        assert_eq!(payload_preview(b""), "");
        let big = vec![b'a'; 64 * 1024];
        assert_eq!(payload_preview(&big).len(), PREVIEW_LEN);
        // A multi-byte character cut at the cap becomes U+FFFD, not a panic.
        let mut cut = vec![b'a'; PREVIEW_LEN - 1];
        cut.extend_from_slice("é".as_bytes());
        assert!(payload_preview(&cut).ends_with('\u{FFFD}'));
    }
}
