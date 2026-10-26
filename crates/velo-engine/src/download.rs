//! Downloads a URL from start to finish: probe, fetch, merge.

use std::path::{Path, PathBuf};

use reqwest::Client;
use tokio::fs;
use tokio_util::sync::CancellationToken;
use velo_core::{Download, DownloadId, Segment, SegmentId, SegmentState};

use crate::fetch::cancellable;
use crate::merge::temp_path;
use crate::{
    Error, Options, Progress, Result, download_segments, fetch_range, merge, planner, probe,
    unique_file_name,
};

/// Downloads `url` into `save_dir` and returns the path of the finished file.
///
/// The file is named from the server's answer (see [`file_name`](crate::file_name)), with
/// ` (1)`, ` (2)`… added if that name is taken, and its parts go in `<temp_dir>/<id>/`. A server
/// that answers range requests gets up to `options.max_connections` connections and resumes from
/// any part files already there. One that doesn't gets a single connection, starting over from
/// the first byte. `progress` is filled with the download's segments once the server has
/// answered, so a caller can watch it.
pub async fn download_file(
    client: &Client,
    url: &str,
    temp_dir: &Path,
    id: DownloadId,
    save_dir: &Path,
    options: &Options,
    progress: &Progress,
) -> Result<PathBuf> {
    let context = RunContext {
        client,
        temp_dir,
        options,
        progress,
        cancel: &CancellationToken::new(),
    };
    let mut download = Download::new(id, url, "", save_dir);
    run(&context, &mut download, &[], |_, _| {}).await
}

/// What one run of a download needs besides the download itself.
pub(crate) struct RunContext<'a> {
    /// The shared HTTP client.
    pub client: &'a Client,
    /// Folder holding each download's part-file folder.
    pub temp_dir: &'a Path,
    /// Connection and retry settings.
    pub options: &'a Options,
    /// Receives the download's segments as they progress.
    pub progress: &'a Progress,
    /// Stops the run with [`Error::Cancelled`](crate::Error::Cancelled).
    pub cancel: &'a CancellationToken,
}

/// A point in a run that the caller hears about through `on_stage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The server answered the probe; the download's metadata is filled in.
    Probed,
    /// Every byte is in; the parts are being joined.
    Merging,
}

/// Runs `download` once: probes the server, downloads what is missing and merges the parts.
/// Returns the path of the finished file.
///
/// `download` is updated as the run goes: its metadata from the probe, its file name (from the
/// server when empty, and made unique at the merge) and its segments. If the server's size,
/// `ETag`, `Last-Modified` or range support changed since the segments were saved, the old part
/// files are deleted and the download starts over, so parts of two versions are never joined.
///
/// With two `streams` (the addresses of a video and its sound), each is downloaded and merged
/// into `<temp>/<id>/stream-<n>` in turn, its parts in `<temp>/<id>/s<n>/` and its segments in
/// `download` while it runs; a stream already merged is kept, so a resume picks up at the
/// stream it stopped in. Then the two are joined into the save path. With fewer, `download.url`
/// is downloaded as one file.
pub(crate) async fn run(
    context: &RunContext<'_>,
    download: &mut Download,
    streams: &[String],
    mut on_stage: impl FnMut(Stage, &Download),
) -> Result<PathBuf> {
    let dir = context.temp_dir.join(download.id.0.to_string());
    if streams.len() < 2 {
        let total = fetch(context, download, &dir, &mut on_stage).await?;
        on_stage(Stage::Merging, download);
        let save_path = claim_save_path(download).await?;
        merge(&dir, &download.segments, total, &save_path).await?;
        return Ok(save_path);
    }

    let mut files = Vec::new();
    for (index, url) in streams.iter().enumerate() {
        let file = dir.join(format!("stream-{index}"));
        if !fs::try_exists(&file).await? {
            download.url.clone_from(url);
            let parts = dir.join(format!("s{index}"));
            let total = fetch(context, download, &parts, &mut on_stage).await?;
            merge(&parts, &download.segments, total, &file).await?;
            // The next stream starts with nothing of its own.
            download.segments.clear();
            download.total_size = None;
            download.etag = None;
            download.last_modified = None;
        }
        files.push(file);
    }
    download.url.clone_from(&streams[0]);
    on_stage(Stage::Merging, download);
    let save_path = claim_save_path(download).await?;
    let joining = temp_path(&save_path);
    let (video, audio, out) = (files[0].clone(), files[1].clone(), joining.clone());
    let joined = tokio::task::spawn_blocking(move || velo_mux::mux(&video, &audio, &out))
        .await
        .map_err(|e| Error::Join(e.to_string()))?;
    if let Err(error) = joined {
        let _ = fs::remove_file(&joining).await;
        return Err(Error::Join(error.to_string()));
    }
    fs::rename(&joining, &save_path).await?;
    download.total_size = Some(fs::metadata(&save_path).await?.len());
    remove_dir(&dir).await?;
    Ok(save_path)
}

/// Downloads `download.url` into part files in `dir`: probes the server, starts over if the
/// file changed, and fetches what is missing. Returns the size from the server.
async fn fetch(
    context: &RunContext<'_>,
    download: &mut Download,
    dir: &Path,
    on_stage: &mut impl FnMut(Stage, &Download),
) -> Result<Option<u64>> {
    let probe = cancellable(context.cancel, probe(context.client, &download.url)).await?;
    let changed = download.total_size != probe.total_size
        || download.resumable != probe.resumable
        || download.etag != probe.etag
        || download.last_modified != probe.last_modified;
    if (changed && !download.segments.is_empty()) || !probe.resumable {
        remove_dir(dir).await?;
        download.segments.clear();
    }
    download.total_size = probe.total_size;
    download.resumable = probe.resumable;
    download.etag = probe.etag;
    download.last_modified = probe.last_modified;
    if download.file_name.is_empty() {
        download.file_name = probe.file_name;
    }
    on_stage(Stage::Probed, download);

    let result = if probe.resumable {
        if download.segments.is_empty() {
            download.segments = planner::initial_segments(probe.total_size.unwrap_or_default());
        }
        context.progress.replace(download.segments.clone());
        download_segments(
            context.client,
            &probe.url,
            dir,
            context.progress,
            context.options,
            context.cancel,
        )
        .await
    } else {
        // Without ranges the only way to fetch is from the first byte.
        let segment = Segment::new(SegmentId(0), 0, 0);
        let part = dir.join(segment.part_file_name());
        context.progress.replace(vec![segment]);
        let progress = context.progress;
        fetch_range(
            context.client,
            &probe.url,
            &part,
            (0, None),
            &context.options.limiters,
            context.cancel,
            |n| {
                progress.update(SegmentId(0), |s| {
                    s.end += n;
                    s.downloaded += n;
                })
            },
        )
        .await
        .map(|len| {
            progress.update(SegmentId(0), |s| {
                (s.end, s.downloaded, s.state) = (len, len, SegmentState::Done)
            })
        })
    };
    download.segments = context.progress.snapshot();
    result?;
    Ok(probe.total_size)
}

/// Creates the save folder and gives `download` a file name that is free there; returns the
/// save path.
async fn claim_save_path(download: &mut Download) -> Result<PathBuf> {
    fs::create_dir_all(&download.save_dir).await?;
    download.file_name = unique_file_name(&download.save_dir, &download.file_name);
    Ok(download.save_path())
}

/// Deletes `dir` and everything in it; a missing folder is fine.
pub(crate) async fn remove_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}
