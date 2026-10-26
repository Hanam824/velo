//! Events the download engine sends to the app.

use serde::{Deserialize, Serialize};

use crate::{Download, DownloadId, Segment};

/// Something that happened in the download engine.
///
/// `Added` and `Changed` carry the whole download, including its segments, so a listener that
/// stores them always has what it needs to resume. `Progress` is sent several times a second
/// for running downloads and is only for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineEvent {
    /// A download was added.
    Added(Download),
    /// A download's state, metadata or saved progress changed.
    Changed(Download),
    /// A download was deleted.
    Removed(DownloadId),
    /// A running download's live progress.
    Progress(ProgressUpdate),
}

/// Live progress of a running download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressUpdate {
    /// The download.
    pub id: DownloadId,
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total size, if known.
    pub total: Option<u64>,
    /// Current transfer rate in bytes per second, smoothed over the last few seconds.
    pub rate: u64,
    /// The segments as they are now.
    pub segments: Vec<Segment>,
}
