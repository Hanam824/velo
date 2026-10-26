//! The segmented download engine: probes a URL, splits it into ranges, downloads each range into a part file and merges the parts.

use std::path::{Path, PathBuf};
use std::sync::Once;

use velo_core::{DownloadId, Segment};

mod download;
mod fetch;
mod filename;
mod merge;
pub mod planner;
mod probe;
mod segmented;

pub use download::download_file;
pub use fetch::fetch_range;
pub use filename::{file_name, sanitize_file_name};
pub use merge::merge;
pub use probe::{Probe, probe};
pub use segmented::{Options, Progress, download_segments};

/// Errors from the download engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The HTTP request failed: bad URL, connection, TLS or protocol error.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Reading or writing a part file failed.
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    /// The server answered with an error status.
    #[error("server answered {0}")]
    Status(reqwest::StatusCode),
    /// The server didn't honour a range request, or answered with a different range.
    #[error("server ignored the range request")]
    RangeIgnored,
    /// The connection ended before the expected number of bytes arrived.
    #[error("connection closed after {received} of {expected} bytes")]
    Incomplete {
        /// Bytes that arrived.
        received: u64,
        /// Bytes that were expected.
        expected: u64,
    },
    /// A merge was asked for before every segment was done, or the segments don't cover the
    /// whole file.
    #[error("the download isn't finished")]
    NotFinished,
    /// A part file's size doesn't match its segment.
    #[error("part file {path} holds {actual} bytes, expected {expected}")]
    PartSize {
        /// The part file.
        path: PathBuf,
        /// The segment's length.
        expected: u64,
        /// The part file's size.
        actual: u64,
    },
    /// The merged file's size doesn't match the server's.
    #[error("merged file holds {actual} bytes, expected {expected}")]
    SizeMismatch {
        /// Size from the server.
        expected: u64,
        /// Size of the merged file.
        actual: u64,
    },
    /// Something already exists at the save path.
    #[error("{0} already exists")]
    Exists(PathBuf),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Builds the HTTP client the engine shares between requests, so connections to a server are
/// reused.
///
/// It verifies certificates against the operating system's trust store, follows redirects and
/// identifies itself as `Velo/<version>`. Responses are never decompressed, so byte offsets
/// match the file on the server.
pub fn client() -> Result<reqwest::Client> {
    static PROVIDER: Once = Once::new();
    PROVIDER.call_once(|| {
        // Fails only if another provider is already installed, which is fine too.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    Ok(reqwest::Client::builder()
        .user_agent(concat!("Velo/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Path of a segment's part file: `<temp_dir>/<download id>/<segment id>.part`.
pub fn part_path(temp_dir: &Path, download: DownloadId, segment: &Segment) -> PathBuf {
    temp_dir
        .join(download.0.to_string())
        .join(segment.part_file_name())
}
