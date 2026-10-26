//! Downloads a file over several connections, one worker per connection, each fetching one
//! segment into its own part file at a time.
//!
//! The segment list is shared behind a mutex. A worker takes its next segment from the
//! [planner](crate::planner), and counts each chunk against its segment under the lock before
//! writing it. The planner reads that count when it splits, so the midpoint it picks is never
//! below bytes a worker has already taken, and a worker whose segment shrank stops at the new
//! end.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::{Client, StatusCode, Url};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::task::JoinSet;
use velo_core::{Segment, SegmentId, SegmentState, Settings};

use crate::fetch::send_range;
use crate::{Error, Result, planner};

/// How a segmented download runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Workers, so connections, at most; at least 1.
    pub max_connections: u32,
    /// Smallest range the planner gives a new worker, in bytes.
    pub min_segment_size: u64,
    /// Retries for a segment after a network error, a cut-off connection or a `408`, `429` or
    /// `5xx` answer. The count starts over whenever an attempt makes progress.
    pub max_retries: u32,
    /// Wait before the first retry; it doubles with each further retry.
    pub retry_delay: Duration,
}

impl Options {
    /// Options from the user's settings, with the default retry policy.
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            max_connections: settings.max_connections,
            min_segment_size: settings.min_segment_size,
            ..Self::default()
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        let settings = Settings::default();
        Self {
            max_connections: settings.max_connections,
            min_segment_size: settings.min_segment_size,
            max_retries: 5,
            retry_delay: Duration::from_secs(1),
        }
    }
}

/// The segment list of a running download, shared between its workers and anyone watching it.
///
/// Cloning gives another handle to the same list.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<Mutex<Vec<Segment>>>);

impl Progress {
    /// Wraps a download's segments, sorted by `start`.
    pub fn new(segments: Vec<Segment>) -> Self {
        Self(Arc::new(Mutex::new(segments)))
    }

    /// A copy of the segments as they are now.
    pub fn snapshot(&self) -> Vec<Segment> {
        self.with(|segments| segments.clone())
    }

    /// Bytes downloaded so far, summed over all segments.
    pub fn downloaded(&self) -> u64 {
        self.with(|segments| segments.iter().map(|s| s.downloaded).sum())
    }

    /// Replaces the whole segment list.
    pub(crate) fn replace(&self, segments: Vec<Segment>) {
        self.with(|current| *current = segments);
    }

    /// Runs `f` with the segment list locked.
    fn with<R>(&self, f: impl FnOnce(&mut Vec<Segment>) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Runs `f` on the segment with `id`, with the list locked.
    pub(crate) fn update<R>(&self, id: SegmentId, f: impl FnOnce(&mut Segment) -> R) -> R {
        self.with(|segments| {
            let segment = segments
                .iter_mut()
                .find(|s| s.id == id)
                .expect("a worker's segment is never removed");
            f(segment)
        })
    }
}

/// Downloads every segment in `progress` from `url` into part files in `dir`, using up to
/// `options.max_connections` connections.
///
/// The server must answer range requests. Before starting, each segment's progress is set from
/// its part file's size on disk, so this resumes whatever is already there; a part file longer
/// than its segment is cut back. On the first error that retries don't fix, the other workers
/// are stopped, active segments go back to pending and the error is returned. Dropping the
/// future stops all workers too.
pub async fn download_segments(
    client: &Client,
    url: &Url,
    dir: &Path,
    progress: &Progress,
    options: &Options,
) -> Result<()> {
    fs::create_dir_all(dir).await?;
    sync_from_disk(dir, progress).await?;

    let worker = Arc::new(Worker {
        client: client.clone(),
        url: url.clone(),
        dir: dir.to_path_buf(),
        progress: progress.clone(),
        options: options.clone(),
    });
    let mut workers = JoinSet::new();
    for _ in 0..options.max_connections.max(1) {
        let worker = Arc::clone(&worker);
        workers.spawn(async move { worker.run().await });
    }
    while let Some(joined) = workers.join_next().await {
        let result = joined.unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
        if let Err(error) = result {
            workers.abort_all();
            progress.with(|segments| {
                for segment in segments.iter_mut() {
                    if segment.state == SegmentState::Active {
                        segment.state = SegmentState::Pending;
                    }
                }
            });
            return Err(error);
        }
    }
    // A worker only stops early on an error, so all segments are done here.
    debug_assert!(progress.with(|s| s.iter().all(|s| s.state == SegmentState::Done)));
    Ok(())
}

/// Sets each segment's progress and state from its part file's size, cutting back part files
/// that are longer than their segment.
async fn sync_from_disk(dir: &Path, progress: &Progress) -> Result<()> {
    for segment in progress.snapshot() {
        let path = dir.join(segment.part_file_name());
        let on_disk = match fs::metadata(&path).await {
            Ok(metadata) => metadata.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };
        if on_disk > segment.len() {
            OpenOptions::new()
                .write(true)
                .open(&path)
                .await?
                .set_len(segment.len())
                .await?;
        }
        progress.update(segment.id, |s| {
            s.downloaded = on_disk.min(s.len());
            s.state = if s.remaining() == 0 {
                SegmentState::Done
            } else {
                SegmentState::Pending
            };
        });
    }
    Ok(())
}

/// What every worker of one download shares.
struct Worker {
    client: Client,
    url: Url,
    dir: PathBuf,
    progress: Progress,
    options: Options,
}

impl Worker {
    /// Fetches segments until the planner has no more work worth a connection.
    async fn run(&self) -> Result<()> {
        loop {
            let next = self
                .progress
                .with(|segments| planner::next_work(segments, self.options.min_segment_size));
            let Some(id) = next else {
                return Ok(());
            };
            self.fetch_with_retries(id).await?;
        }
    }

    /// Fetches segment `id`, retrying errors that may go away with exponential backoff.
    async fn fetch_with_retries(&self, id: SegmentId) -> Result<()> {
        let mut failures = 0;
        loop {
            let before = self.progress.update(id, |s| s.downloaded);
            let error = match self.fetch(id).await {
                Ok(()) => return Ok(()),
                Err(error) => error,
            };
            if self.progress.update(id, |s| s.downloaded) > before {
                failures = 0;
            }
            if !is_retryable(&error) || failures >= self.options.max_retries {
                return Err(error);
            }
            tokio::time::sleep(self.options.retry_delay * 2u32.saturating_pow(failures)).await;
            failures += 1;
        }
    }

    /// Fetches segment `id` from its current position to its current end, stopping early if the
    /// planner shrinks the end, and marks it done.
    async fn fetch(&self, id: SegmentId) -> Result<()> {
        let path = self
            .dir
            .join(self.progress.update(id, |s| s.part_file_name()));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .await?;
        // The part file is the source of truth: a failed write may have been counted already.
        let on_disk = file.metadata().await?.len();
        let (have, position, end) = self.progress.update(id, |s| {
            s.downloaded = on_disk.min(s.len());
            (s.downloaded, s.position(), s.end)
        });
        if on_disk > have {
            file.set_len(have).await?;
        }
        if position >= end {
            self.progress.update(id, |s| s.state = SegmentState::Done);
            return Ok(());
        }
        file.seek(SeekFrom::Start(have)).await?;

        let mut response = send_range(&self.client, &self.url, position, Some(end)).await?;
        let mut writer = BufWriter::new(file);
        while let Some(chunk) = response.chunk().await? {
            let (take, done) = self.progress.update(id, |s| {
                let take = chunk.len().min(s.remaining() as usize);
                s.downloaded += take as u64;
                let done = s.remaining() == 0;
                if done {
                    s.state = SegmentState::Done;
                }
                (take, done)
            });
            writer.write_all(&chunk[..take]).await?;
            if done {
                writer.flush().await?;
                return Ok(());
            }
        }
        writer.flush().await?;
        let (received, expected) = self.progress.update(id, |s| (s.downloaded, s.len()));
        Err(Error::Incomplete { received, expected })
    }
}

/// Whether trying again later might fix `error`.
fn is_retryable(error: &Error) -> bool {
    match error {
        Error::Http(e) => !e.is_builder() && !e.is_redirect(),
        Error::Incomplete { .. } => true,
        Error::Status(status) => {
            status.is_server_error()
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
        }
        _ => false,
    }
}
