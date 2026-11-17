//! Downloads a file over several connections, one worker per connection, each fetching one
//! segment at a time into its own range of one shared data file.
//!
//! The segment list is shared behind a mutex. A worker takes its next segment from the
//! [planner](crate::planner), and counts each chunk against its segment under the lock before
//! writing it. The planner reads that count when it splits, so the midpoint it picks is never
//! below bytes a worker has already taken, and a worker whose segment shrank stops at the new
//! end.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode, Url};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use velo_core::{Segment, SegmentId, SegmentState, Settings};

use crate::fetch::{WRITE_BUFFER, cancellable, send_range};
use crate::{Error, Limiter, Result, planner};

/// How a segmented download runs.
#[derive(Debug, Clone)]
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
    /// Speed limits every chunk must pass, such as the global one and the download's own.
    pub limiters: Vec<Limiter>,
    /// The most bytes one request asks for; a longer segment is fetched in several requests
    /// over the same connection. Some servers (such as YouTube's) slow down or refuse longer
    /// ranges. No cap if `None`.
    pub max_request_size: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        let settings = Settings::default();
        Self {
            max_connections: settings.max_connections,
            min_segment_size: settings.min_segment_size,
            max_retries: 5,
            retry_delay: Duration::from_secs(1),
            limiters: Vec::new(),
            max_request_size: None,
        }
    }
}

/// The segment list of a running download, shared between its workers and anyone watching it.
///
/// Each segment's `downloaded` counts bytes as they arrive. Beside it, `Progress` keeps how many
/// of them a worker has handed to the OS (written) and how many of those a sync has put on disk
/// (durable): [`durable_snapshot`](Self::durable_snapshot) reports the durable ones, and only
/// those may be saved for a resume, since the data file's size says nothing about them.
///
/// Cloning gives another handle to the same list.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<Mutex<State>>);

/// What a [`Progress`] guards.
#[derive(Debug, Default)]
struct State {
    /// The segments, sorted by `start`.
    segments: Vec<Segment>,
    /// Bytes of each segment handed to the OS, not just buffered; a missing segment has none.
    written: HashMap<SegmentId, u64>,
    /// Bytes of each segment synced to disk; a segment that is missing has none.
    durable: HashMap<SegmentId, u64>,
    /// How fast each active segment's worker receives, for the planner to pick which to split.
    rates: HashMap<SegmentId, Rate>,
    /// Workers still running. Changed only under the same lock as the planner, so a worker
    /// that hands its segment back can tell whether another one is left to take it.
    workers: usize,
}

/// A worker's receiving speed, averaged over about [`RATE_WINDOW`].
#[derive(Debug, Clone, Copy)]
struct Rate {
    /// When the current sample started.
    since: Instant,
    /// Bytes received since `since`.
    bytes: u64,
    /// Bytes a second; `None` until the first sample is taken.
    per_second: Option<f64>,
}

/// How far back a worker's speed looks.
const RATE_WINDOW: Duration = Duration::from_secs(3);

/// Shortest time a speed sample spans, so chunks that arrive together don't read as a burst.
const RATE_SAMPLE: Duration = Duration::from_millis(200);

impl Rate {
    /// A rate with no reading yet, sampling from now.
    fn start() -> Self {
        Self {
            since: Instant::now(),
            bytes: 0,
            per_second: None,
        }
    }

    /// Counts `bytes` that arrived just now, and folds the sample into the average once it
    /// spans [`RATE_SAMPLE`].
    fn record(&mut self, bytes: u64) {
        self.bytes += bytes;
        let elapsed = self.since.elapsed();
        if elapsed < RATE_SAMPLE {
            return;
        }
        let elapsed = elapsed.as_secs_f64();
        let sample = self.bytes as f64 / elapsed;
        // An exponential average whose weight follows the time passed, not the chunk count.
        let weight = 1.0 - (-elapsed / RATE_WINDOW.as_secs_f64()).exp();
        self.per_second = Some(
            self.per_second
                .map_or(sample, |p| p + weight * (sample - p)),
        );
        self.restart();
    }

    /// Starts a new sample from now, keeping the average, so time spent waiting to retry or
    /// to connect doesn't count as slow receiving.
    fn restart(&mut self) {
        self.since = Instant::now();
        self.bytes = 0;
    }
}

/// The segment with `id` in `segments`; a worker's segment is always there.
fn segment_mut(segments: &mut [Segment], id: SegmentId) -> &mut Segment {
    segments
        .iter_mut()
        .find(|s| s.id == id)
        .expect("a worker's segment is never removed")
}

impl State {
    /// Takes `segments` as they are, every counted byte taken as synced.
    fn new(segments: Vec<Segment>) -> Self {
        let durable: HashMap<_, _> = segments.iter().map(|s| (s.id, s.downloaded)).collect();
        Self {
            segments,
            written: durable.clone(),
            durable,
            rates: HashMap::new(),
            workers: 0,
        }
    }
}

impl Progress {
    /// Wraps a download's segments, sorted by `start`; their `downloaded` bytes must be on disk.
    pub fn new(segments: Vec<Segment>) -> Self {
        Self(Arc::new(Mutex::new(State::new(segments))))
    }

    /// A copy of the segments as they are now.
    pub fn snapshot(&self) -> Vec<Segment> {
        self.with(|segments| segments.clone())
    }

    /// A copy of the segments counting only bytes synced to disk: what to save for a resume.
    /// A done segment that isn't fully synced yet is pending.
    pub fn durable_snapshot(&self) -> Vec<Segment> {
        self.lock(|state| {
            state
                .segments
                .iter()
                .map(|s| {
                    let synced = state.durable.get(&s.id).copied().unwrap_or(0).min(s.len());
                    let mut segment = s.clone();
                    segment.downloaded = synced;
                    if segment.remaining() > 0 {
                        segment.state = SegmentState::Pending;
                    } else {
                        segment.state = SegmentState::Done;
                    }
                    segment
                })
                .collect()
        })
    }

    /// Bytes downloaded so far, summed over all segments.
    pub fn downloaded(&self) -> u64 {
        self.with(|segments| segments.iter().map(|s| s.downloaded).sum())
    }

    /// Replaces the whole segment list; their `downloaded` bytes must be on disk.
    pub(crate) fn replace(&self, segments: Vec<Segment>) {
        self.lock(|state| *state = State::new(segments));
    }

    /// Runs `f` with everything locked.
    fn lock<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Runs `f` with the segment list locked.
    fn with<R>(&self, f: impl FnOnce(&mut Vec<Segment>) -> R) -> R {
        self.lock(|state| f(&mut state.segments))
    }

    /// Runs `f` on the segment with `id`, with the list locked.
    pub(crate) fn update<R>(&self, id: SegmentId, f: impl FnOnce(&mut Segment) -> R) -> R {
        self.with(|segments| f(segment_mut(segments, id)))
    }

    /// If another worker is still running, hands segment `id` back as pending (unless it is
    /// done) for that one to take, counts the caller as stopped and returns `true`; the caller
    /// must then stop. Otherwise changes nothing and returns `false`.
    fn hand_back(&self, id: SegmentId) -> bool {
        self.lock(|state| {
            if state.workers <= 1 {
                return false;
            }
            state.workers -= 1;
            state.rates.remove(&id);
            let segment = segment_mut(&mut state.segments, id);
            if segment.state == SegmentState::Active {
                segment.state = SegmentState::Pending;
            }
            true
        })
    }

    /// Starts a new speed sample for segment `id`, as its worker starts receiving again.
    fn restart_rate(&self, id: SegmentId) {
        self.lock(|state| {
            if let Some(rate) = state.rates.get_mut(&id) {
                rate.restart();
            }
        });
    }

    /// Records that segment `id`'s counted bytes are all handed to the OS.
    fn written(&self, id: SegmentId) {
        self.lock(|state| {
            if let Some(s) = state.segments.iter().find(|s| s.id == id) {
                state.written.insert(id, s.downloaded);
            }
        });
    }

    /// The written bytes of each segment, taken before a sync so that only those are promoted.
    fn written_snapshot(&self) -> HashMap<SegmentId, u64> {
        self.lock(|state| state.written.clone())
    }

    /// Counts the bytes in `written`, taken before a sync that has now finished, as durable.
    fn promote(&self, written: HashMap<SegmentId, u64>) {
        self.lock(|state| {
            for (id, bytes) in written {
                let durable = state.durable.entry(id).or_default();
                *durable = (*durable).max(bytes);
            }
        });
    }

    /// Forgets segment `id`'s bytes that were counted but never handed to the OS (a failed
    /// write, or a buffer dropped on an error), so they are fetched again; returns how many are
    /// left. Written bytes stay: they are in the file for this run, and the next sync makes
    /// them durable.
    fn roll_back(&self, id: SegmentId) -> u64 {
        self.lock(|state| {
            let written = state.written.get(&id).copied().unwrap_or(0);
            let segment = segment_mut(&mut state.segments, id);
            segment.downloaded = written.min(segment.len());
            segment.downloaded
        })
    }
}

/// Downloads every segment in `progress` from `url` into `file`, each at its own offset, using
/// up to `options.max_connections` connections.
///
/// The server must answer range requests. `file` is created if missing and given `total` bytes
/// (sparse, see [`velo_platform::preallocate`]); the bytes each segment counts as downloaded
/// must already be in it, so this resumes from them. The file is synced once a second, and once
/// more when the workers stop; only bytes written before a sync count as durable.
///
/// Cancelling `cancel` stops the download with [`Error::Cancelled`]. On that, or on the first
/// error that retries don't fix, every worker is stopped and waited for, so nothing writes to the
/// file once this returns; active segments go back to pending.
pub async fn download_segments(
    client: &Client,
    url: &Url,
    file: &Path,
    total: u64,
    progress: &Progress,
    options: &Options,
    cancel: &CancellationToken,
) -> Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).await?;
    }
    let data = Arc::new(
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(file)
            .await?
            .into_std()
            .await,
    );
    if data.metadata()?.len() != total {
        let data = Arc::clone(&data);
        tokio::task::spawn_blocking(move || velo_platform::preallocate(&data, total))
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }

    let worker = Arc::new(Worker {
        client: client.clone(),
        url: url.clone(),
        file: file.to_path_buf(),
        progress: progress.clone(),
        options: options.clone(),
        stop: cancel.child_token(),
    });
    let mut workers = JoinSet::new();
    progress.lock(|state| state.workers = options.max_connections.max(1) as usize);
    for _ in 0..options.max_connections.max(1) {
        let worker = Arc::clone(&worker);
        workers.spawn(async move { worker.run().await });
    }
    let mut first_error = None;
    // One sync for every worker: syncing any handle puts the whole file's written bytes on disk.
    let start = tokio::time::Instant::now() + SYNC_INTERVAL;
    let mut tick = tokio::time::interval_at(start, SYNC_INTERVAL);
    // A sync slower than the interval (a slow disk) mustn't be followed by a burst of them.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            joined = workers.join_next() => {
                let Some(joined) = joined else { break };
                let result = joined.unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
                if let Err(error) = result {
                    // Stop the others, but keep waiting for them so none is still writing.
                    worker.stop.cancel();
                    first_error.get_or_insert(error);
                }
            }
            _ = tick.tick() => {
                if let Err(error) = sync(&data, progress).await {
                    worker.stop.cancel();
                    first_error.get_or_insert(error);
                }
            }
        }
    }
    // Everything the workers wrote, so a finished or paused download saves all of it.
    if let Err(error) = sync(&data, progress).await {
        first_error.get_or_insert(error);
    }
    if let Some(error) = first_error {
        progress.with(|segments| {
            for segment in segments.iter_mut() {
                if segment.state == SegmentState::Active {
                    segment.state = SegmentState::Pending;
                }
            }
        });
        // Report the user's stop rather than an error it caused in another worker.
        return Err(if cancel.is_cancelled() {
            Error::Cancelled
        } else {
            error
        });
    }
    // Workers stop early only on an error, or after handing their segment to a worker still
    // running (`State::workers`), so all segments are done here.
    debug_assert!(progress.with(|s| s.iter().all(|s| s.state == SegmentState::Done)));
    Ok(())
}

/// How often the data file is synced to disk.
const SYNC_INTERVAL: Duration = Duration::from_secs(1);

/// Syncs `data` to disk and counts every byte written before the sync as durable.
async fn sync(data: &Arc<std::fs::File>, progress: &Progress) -> Result<()> {
    let written = progress.written_snapshot();
    let file = Arc::clone(data);
    tokio::task::spawn_blocking(move || file.sync_data())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .inspect_err(|error| tracing::warn!(%error, "couldn't sync a download file"))?;
    progress.promote(written);
    Ok(())
}

/// What every worker of one download shares.
struct Worker {
    client: Client,
    url: Url,
    /// The data file every worker writes into.
    file: PathBuf,
    progress: Progress,
    options: Options,
    /// Cancelled when the download is stopped or a worker fails.
    stop: CancellationToken,
}

impl Worker {
    /// Fetches segments until the planner has no more work worth a connection.
    async fn run(&self) -> Result<()> {
        loop {
            if self.stop.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let next = self.progress.lock(|state| {
                let rates = &state.rates;
                let next =
                    planner::next_work(&mut state.segments, self.options.min_segment_size, |id| {
                        rates.get(&id).and_then(|r| r.per_second)
                    });
                if next.is_none() {
                    state.workers -= 1;
                }
                next
            });
            let Some(id) = next else {
                return Ok(());
            };
            if !self.fetch_with_retries(id).await? {
                return Ok(());
            }
        }
    }

    /// Fetches segment `id`, retrying errors that may go away with exponential backoff, and
    /// returns `true` once it is done.
    ///
    /// When the server refuses the connection (`503` or `429`) while another worker is still
    /// running, as a server that allows fewer connections than Velo opens does, the segment is
    /// handed to that worker at once and this returns `false`: the caller stops.
    async fn fetch_with_retries(&self, id: SegmentId) -> Result<bool> {
        let mut failures = 0;
        loop {
            let before = self.progress.update(id, |s| s.downloaded);
            let error = match self.fetch(id).await {
                Ok(()) => return Ok(true),
                Err(error) => error,
            };
            if is_refusal(&error) && self.progress.hand_back(id) {
                tracing::warn!(segment = id.0, %error, "dropping a connection the server refuses");
                return Ok(false);
            }
            if self.progress.update(id, |s| s.downloaded) > before {
                failures = 0;
            }
            if !is_retryable(&error) || failures >= self.options.max_retries {
                return Err(error);
            }
            let delay = self.options.retry_delay * 2u32.saturating_pow(failures);
            tracing::warn!(segment = id.0, %error, failures, ?delay, "retrying a segment");
            cancellable(&self.stop, async {
                tokio::time::sleep(delay).await;
                Ok(())
            })
            .await?;
            failures += 1;
        }
    }

    /// Fetches segment `id` from its current position to its current end, stopping early if the
    /// planner shrinks the end, and marks it done.
    async fn fetch(&self, id: SegmentId) -> Result<()> {
        // Bytes counted but never written may not be in the file: fetch them again.
        let have = self.progress.roll_back(id);
        let (start, position, end) = self.progress.update(id, |s| (s.start, s.position(), s.end));
        if position >= end {
            self.progress.update(id, |s| s.state = SegmentState::Done);
            return Ok(());
        }
        let mut file = OpenOptions::new().write(true).open(&self.file).await?;
        file.seek(SeekFrom::Start(start + have)).await?;
        let mut writer = BufWriter::with_capacity(WRITE_BUFFER, file);
        let mut unflushed = 0;
        loop {
            let (position, end) = self.progress.update(id, |s| (s.position(), s.end));
            let request_end = match self.options.max_request_size {
                Some(max) => end.min(position.saturating_add(max.max(1))),
                None => end,
            };
            let sent = Instant::now();
            let result = self
                .fetch_request(id, &mut writer, &mut unflushed, position, request_end)
                .await;
            tracing::debug!(
                segment = id.0,
                position,
                asked = request_end - position,
                received = self.progress.update(id, |s| s.position()) - position,
                elapsed = ?sent.elapsed(),
                ok = result.is_ok(),
                "range request ended"
            );
            match result {
                Ok(true) => return self.flush(id, &mut writer).await,
                Ok(false) => {}
                Err(error) => {
                    // A failed write leaves counted bytes that aren't in the file; anything
                    // else (a network error, a stop) leaves good bytes worth keeping.
                    if !matches!(error, Error::Io(_)) {
                        let _ = self.flush(id, &mut writer).await;
                    }
                    return Err(error);
                }
            }
        }
    }

    /// Hands what `writer` holds to the OS, then counts segment `id`'s bytes as written.
    async fn flush(&self, id: SegmentId, writer: &mut BufWriter<tokio::fs::File>) -> Result<()> {
        writer.flush().await?;
        self.progress.written(id);
        Ok(())
    }

    /// Fetches bytes `[position, request_end)` of segment `id` with one request into
    /// `writer`. Returns `true` once the segment is done (its end may have shrunk meanwhile)
    /// and `false` when this request's range is in but the segment isn't.
    async fn fetch_request(
        &self,
        id: SegmentId,
        writer: &mut BufWriter<tokio::fs::File>,
        unflushed: &mut usize,
        position: u64,
        request_end: u64,
    ) -> Result<bool> {
        let mut response = cancellable(
            &self.stop,
            send_range(&self.client, &self.url, position, Some(request_end)),
        )
        .await?;
        self.progress.restart_rate(id);
        let mut left = request_end - position;
        while left > 0 {
            let Some(chunk) =
                cancellable(&self.stop, async { Ok(response.chunk().await?) }).await?
            else {
                break;
            };
            let wanted = (chunk.len() as u64)
                .min(left)
                .min(self.progress.update(id, |s| s.remaining()));
            for limiter in &self.options.limiters {
                cancellable(&self.stop, async {
                    limiter.acquire(wanted).await;
                    Ok(())
                })
                .await?;
            }
            let (take, done) = self.progress.lock(|state| {
                let s = segment_mut(&mut state.segments, id);
                let take = (chunk.len() as u64).min(left).min(s.remaining()) as usize;
                s.downloaded += take as u64;
                let done = s.remaining() == 0;
                if done {
                    s.state = SegmentState::Done;
                    state.rates.remove(&id);
                } else {
                    state
                        .rates
                        .entry(id)
                        .and_modify(|r| r.record(take as u64))
                        .or_insert_with(Rate::start);
                }
                (take, done)
            });
            writer.write_all(&chunk[..take]).await?;
            left -= take as u64;
            *unflushed += take;
            if *unflushed >= WRITE_BUFFER {
                self.flush(id, writer).await?;
                *unflushed = 0;
            }
            if done {
                return Ok(true);
            }
        }
        if left == 0 {
            return Ok(false);
        }
        let (received, expected) = self.progress.update(id, |s| (s.downloaded, s.len()));
        Err(Error::Incomplete { received, expected })
    }
}

/// Whether `error` is the server turning this connection away for now, as a server that limits
/// connections per client does.
fn is_refusal(error: &Error) -> bool {
    matches!(
        error,
        Error::Status(StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS)
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bytes_written_before_a_sync_become_durable() {
        let progress = Progress::new(vec![Segment::new(SegmentId(0), 0, 100)]);
        progress.update(SegmentId(0), |s| s.downloaded = 30);
        progress.written(SegmentId(0));
        let before_sync = progress.written_snapshot();
        // More bytes are written while the sync runs; they aren't covered by it.
        progress.update(SegmentId(0), |s| s.downloaded = 60);
        progress.written(SegmentId(0));
        progress.promote(before_sync);
        assert_eq!(progress.durable_snapshot()[0].downloaded, 30);

        // Counted but never written (a failed write): a retry starts after the written bytes.
        progress.update(SegmentId(0), |s| s.downloaded = 90);
        assert_eq!(progress.roll_back(SegmentId(0)), 60);
    }
}
