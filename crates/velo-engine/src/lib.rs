//! The segmented download engine: probes a URL, splits it into ranges, downloads each range into a part file and merges the parts.

use std::path::{Path, PathBuf};
use std::sync::Once;

use velo_core::{DownloadId, Segment};

mod download;
mod engine;
mod fetch;
mod filename;
mod merge;
pub mod planner;
mod probe;
mod segmented;

pub use download::download_file;
pub use engine::{Engine, EngineConfig, NewDownload};
pub use fetch::fetch_range;
pub use filename::{file_name, sanitize_file_name, unique_file_name};
pub use merge::merge;
pub use probe::{Probe, probe};
pub use reqwest::Url;
pub use segmented::{Options, Progress, download_segments};
pub use tokio_util::sync::CancellationToken;

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
    /// The download was stopped by its caller.
    #[error("the download was stopped")]
    Cancelled,
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
    client_for(None)
}

/// Builds a client like [`client`] that also sends `credentials`, if any, with every request
/// as HTTP Basic authorization.
pub fn client_for(credentials: Option<&Credentials>) -> Result<reqwest::Client> {
    static PROVIDER: Once = Once::new();
    PROVIDER.call_once(|| {
        // Fails only if another provider is already installed, which is fine too.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(credentials) = credentials {
        let mut value = credentials.basic_header();
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    Ok(reqwest::Client::builder()
        .user_agent(concat!("Velo/", env!("CARGO_PKG_VERSION")))
        .default_headers(headers)
        .build()?)
}

/// A user name and password for a server that asks for them.
///
/// The engine keeps these in memory only; they are never written to the download list.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The user name.
    pub user: String,
    /// The password.
    pub password: String,
}

impl Credentials {
    /// The `Authorization: Basic …` header value.
    fn basic_header(&self) -> reqwest::header::HeaderValue {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.user, self.password));
        reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
            .expect("base64 is a valid header value")
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &"…")
            .finish()
    }
}

/// Path of a segment's part file: `<temp_dir>/<download id>/<segment id>.part`.
pub fn part_path(temp_dir: &Path, download: DownloadId, segment: &Segment) -> PathBuf {
    temp_dir
        .join(download.0.to_string())
        .join(segment.part_file_name())
}
