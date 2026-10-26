//! A download: one URL saved to one file, with its server metadata and segments.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Segment;

/// Identifies a download; also names its temp directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DownloadId(pub u64);

/// Where a download is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadState {
    /// Added but not started, or waiting in a queue.
    Queued,
    /// Asking the server for the size, range support and file name.
    Probing,
    /// Workers are fetching segments.
    Downloading,
    /// Stopped by the user; can be resumed.
    Paused,
    /// All segments are done and the part files are being joined.
    Merging,
    /// The file is at its save path.
    Complete,
    /// Stopped by an error, with a message for the user.
    Failed(String),
}

/// One URL saved to one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    /// The download's id.
    pub id: DownloadId,
    /// The URL being fetched.
    pub url: String,
    /// File name at the save path.
    pub file_name: String,
    /// Directory the finished file goes into.
    pub save_dir: PathBuf,
    /// Total size from the server, if it sent one.
    pub total_size: Option<u64>,
    /// Whether the server accepts range requests, so the download can be split and resumed.
    pub resumable: bool,
    /// The server's `ETag`, used to detect a changed file on resume.
    pub etag: Option<String>,
    /// The server's `Last-Modified`, used to detect a changed file on resume.
    pub last_modified: Option<String>,
    /// Name of the [`Category`](crate::Category) the download is filed under.
    pub category: Option<String>,
    /// Free-text description shown in the list.
    pub description: String,
    /// Lifecycle state.
    pub state: DownloadState,
    /// The byte ranges, sorted by `start`; empty until the download has been probed.
    pub segments: Vec<Segment>,
    /// When the download was added, in seconds since the Unix epoch.
    pub added_at: Option<i64>,
    /// When the download was last started, in seconds since the Unix epoch.
    pub last_try_at: Option<i64>,
    /// What the address is worked out from before each start, if it expires.
    #[serde(default)]
    pub source: Option<crate::Source>,
}

impl Download {
    /// Creates a queued download of `url` that has not been probed yet.
    pub fn new(
        id: DownloadId,
        url: impl Into<String>,
        file_name: impl Into<String>,
        save_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            file_name: file_name.into(),
            save_dir: save_dir.into(),
            total_size: None,
            resumable: false,
            etag: None,
            last_modified: None,
            category: None,
            description: String::new(),
            state: DownloadState::Queued,
            segments: Vec::new(),
            added_at: None,
            last_try_at: None,
            source: None,
        }
    }

    /// Full path of the finished file.
    pub fn save_path(&self) -> PathBuf {
        self.save_dir.join(&self.file_name)
    }

    /// Whether the download completed and its file is at the save path.
    pub fn is_finished(&self) -> bool {
        self.state == DownloadState::Complete
    }

    /// Whether the download is being probed, fetched or merged.
    pub fn is_running(&self) -> bool {
        matches!(
            self.state,
            DownloadState::Probing | DownloadState::Downloading | DownloadState::Merging
        )
    }

    /// Bytes downloaded so far, summed over all segments.
    pub fn downloaded(&self) -> u64 {
        self.segments.iter().map(|s| s.downloaded).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SegmentId;

    #[test]
    fn downloaded_sums_the_segments() {
        let mut download =
            Download::new(DownloadId(1), "http://example.test/a.zip", "a.zip", "/tmp");
        let mut first = Segment::new(SegmentId(0), 0, 10);
        first.downloaded = 4;
        let mut second = Segment::new(SegmentId(1), 10, 20);
        second.downloaded = 3;
        download.segments = vec![first, second];
        assert_eq!(download.downloaded(), 7);
        assert_eq!(download.save_path(), PathBuf::from("/tmp").join("a.zip"));
    }
}
