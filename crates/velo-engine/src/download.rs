//! Downloads a URL from start to finish: probe, fetch, finish.

use std::path::{Path, PathBuf};

use reqwest::Client;
use tokio::fs;
use tokio_util::sync::CancellationToken;
use velo_core::{Download, DownloadId, Segment, SegmentId, SegmentState};

use crate::fetch::cancellable;
use crate::{
    Error, Options, Progress, Result, download_segments, fetch_range, finish, planner, probe,
    temp_path, unique_file_name,
};

/// Downloads `url` into `save_dir` and returns the path of the finished file.
///
/// The file is named from the server's answer (see [`file_name`](crate::file_name)), with
/// ` (1)`, ` (2)`… added if that name is taken, and is written to `<name>.velo-tmp` beside it
/// until done. A server that answers range requests gets up to `options.max_connections`
/// connections. One that doesn't gets a single connection, starting over from the first byte.
/// `progress` is filled with the download's segments once the server has answered, so a caller
/// can watch it.
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
    /// Folder holding each download's own folder, for the streams of a video.
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
    /// The server answered the probe; the download's metadata and file name are filled in.
    Probed,
    /// Every byte is in; the file is being checked, or a video's streams joined.
    Merging,
}

/// Runs `download` once: probes the server, downloads what is missing and moves the file into
/// place. Returns the path of the finished file.
///
/// `download` is updated as the run goes: its metadata from the probe, its file name (from the
/// server when empty, made unique right after the probe) and its segments, counting only bytes
/// synced to disk when the run fails. The file is written to `<save path>.velo-tmp`. If the
/// server's size, `ETag`, `Last-Modified` or range support changed since the segments were
/// saved, or that file is gone, the download starts over, so bytes of two versions are never
/// mixed.
///
/// With two `streams` (the addresses of a video and its sound), each is downloaded into
/// `<temp>/<id>/stream-<n>` in turn, its segments in `download` while it runs; a stream already
/// finished is kept, so a resume picks up at the stream it stopped in. Then the two are joined
/// into the save path. With fewer, `download.url` is downloaded as one file.
pub(crate) async fn run(
    context: &RunContext<'_>,
    download: &mut Download,
    streams: &[String],
    mut on_stage: impl FnMut(Stage, &Download),
) -> Result<PathBuf> {
    let dir = context.temp_dir.join(download.id.0.to_string());
    if streams.len() < 2 {
        // Part files from versions that joined them at the end; they can't be resumed.
        remove_dir(&dir).await?;
        let (total, data) = fetch(context, download, None, &mut on_stage).await?;
        on_stage(Stage::Merging, download);
        let save_path = download.save_path();
        finish(&data, &download.segments, total, &save_path).await?;
        return Ok(save_path);
    }

    let mut files = Vec::new();
    for (index, url) in streams.iter().enumerate() {
        let file = dir.join(format!("stream-{index}"));
        if !fs::try_exists(&file).await? {
            download.url.clone_from(url);
            let data = temp_path(&file);
            let (total, _) = fetch(context, download, Some(&data), &mut on_stage).await?;
            finish(&data, &download.segments, total, &file).await?;
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

/// Downloads `download.url` into the data file `data`, or with `None` into
/// `<save path>.velo-tmp` after claiming a save path: probes the server, starts over if the file
/// changed, and fetches what is missing. Returns the size from the server and the data file.
async fn fetch(
    context: &RunContext<'_>,
    download: &mut Download,
    data: Option<&Path>,
    on_stage: &mut impl FnMut(Stage, &Download),
) -> Result<(Option<u64>, PathBuf)> {
    let probe = cancellable(context.cancel, probe(context.client, &download.url)).await?;
    let changed = download.total_size != probe.total_size
        || download.resumable != probe.resumable
        || download.etag != probe.etag
        || download.last_modified != probe.last_modified;
    download.total_size = probe.total_size;
    download.resumable = probe.resumable;
    download.etag = probe.etag;
    download.last_modified = probe.last_modified;
    if download.file_name.is_empty() {
        download.file_name = probe.file_name;
    }
    let data = match data {
        Some(data) => data.to_path_buf(),
        None => {
            // A resume keeps its name while its data file is there; otherwise claim a free one.
            let owned = !download.segments.is_empty()
                && fs::try_exists(temp_path(&download.save_path())).await?;
            if owned && fs::try_exists(download.save_path()).await? {
                // Something else took the save path: keep the bytes under the next free name.
                let wanted = download.file_name.clone();
                move_claim(download, &wanted)?;
            } else if !owned {
                claim_save_path(download).await?;
                // The claimed file is new and empty: saved progress isn't in it.
                download.segments.clear();
            }
            temp_path(&download.save_path())
        }
    };
    if (changed && !download.segments.is_empty())
        || !probe.resumable
        || !fs::try_exists(&data).await?
    {
        // Emptied in place, not deleted, so the claimed name stays reserved.
        if let Some(dir) = data.parent() {
            fs::create_dir_all(dir).await?;
        }
        fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&data)
            .await?;
        download.segments.clear();
    }
    // Segments go out with the claimed name, so a crash from here on resumes into this data
    // file (and a delete removes it) instead of claiming another name and leaving it behind.
    if download.segments.is_empty() {
        download.segments = if probe.resumable {
            planner::initial_segments(probe.total_size.unwrap_or_default())
        } else {
            vec![Segment::new(SegmentId(0), 0, 0)]
        };
    }
    // Before `Probed`: from then on the engine saves this progress, and the old one may count
    // bytes that this file doesn't hold.
    context.progress.replace(download.segments.clone());
    on_stage(Stage::Probed, download);

    let result = if probe.resumable {
        download_segments(
            context.client,
            &probe.url,
            &data,
            probe.total_size.unwrap_or_default(),
            context.progress,
            context.options,
            context.cancel,
        )
        .await
    } else {
        // Without ranges the only way to fetch is from the first byte (its file was emptied).
        let progress = context.progress;
        fetch_range(
            context.client,
            &probe.url,
            &data,
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
    // After a failure only bytes synced to disk may be saved for a resume.
    download.segments = match result {
        Ok(()) => context.progress.snapshot(),
        Err(_) => context.progress.durable_snapshot(),
    };
    result?;
    Ok((probe.total_size, data))
}

/// Creates the save folder and gives `download` a file name that is free there; returns the
/// save path.
///
/// The name is reserved by creating its empty `<name>.velo-tmp` with `create_new`, so two
/// downloads claiming at once never get the same name.
async fn claim_save_path(download: &mut Download) -> Result<PathBuf> {
    fs::create_dir_all(&download.save_dir).await?;
    download.file_name = reserve(&download.save_dir, &download.file_name)?;
    Ok(download.save_path())
}

/// Moves the unfinished file of `download`, which it has claimed, to a free name based on
/// `wanted` and gives the download that name; its segments stay valid.
///
/// Used when the user renames a paused download and when something else took its save path.
/// On an error the download keeps its old name and file.
pub(crate) fn move_claim(download: &mut Download, wanted: &str) -> Result<()> {
    let old = temp_path(&download.save_path());
    let name = reserve(&download.save_dir, wanted)?;
    let new = temp_path(&download.save_dir.join(&name));
    // Replaces the empty placeholder `reserve` made, on every OS.
    if let Err(error) = std::fs::rename(&old, &new) {
        let _ = std::fs::remove_file(&new);
        tracing::warn!(%error, "couldn't move an unfinished download to its new name");
        return Err(error.into());
    }
    download.file_name = name;
    Ok(())
}

/// Reserves the first free name in `dir` based on `wanted` (see [`unique_file_name`]) by
/// creating its empty `<name>.velo-tmp` with `create_new`, so two downloads reserving at once
/// never get the same name, and returns it.
fn reserve(dir: &Path, wanted: &str) -> Result<String> {
    loop {
        // Numbered from `wanted`, so a lost race gives `a (2).bin`, not `a (1) (1).bin`.
        let name = unique_file_name(dir, wanted);
        let path = temp_path(&dir.join(&name));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(name),
            // Another download took it between the check and the create: try the next name.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            // ponytail: Windows refuses (PermissionDenied) a name whose file is deleted but still
            // open, and hides it from `exists`, so it can't be skipped here; the download fails
            // and a retry works once that handle closes. Number past it if this shows up.
            Err(e) => return Err(e.into()),
        }
    }
}

/// Deletes `file`; a missing file is fine.
pub(crate) async fn remove_file(file: &Path) -> Result<()> {
    match fs::remove_file(file).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Deletes `dir` and everything in it; a missing folder is fine.
pub(crate) async fn remove_dir(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(dir).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use velo_core::DownloadId;

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_claiming_the_same_name_at_once_get_different_ones() {
        let dir = tempfile::tempdir().unwrap();
        let claims = (0..8).map(|n| {
            let mut download = Download::new(DownloadId(n), "http://x/", "data.bin", dir.path());
            tokio::spawn(async move { claim_save_path(&mut download).await.unwrap() })
        });
        let mut paths = Vec::new();
        for claim in claims {
            paths.push(claim.await.unwrap());
        }
        paths.sort();
        let mut expected: Vec<_> = (1..8)
            .map(|n| dir.path().join(format!("data ({n}).bin")))
            .collect();
        expected.push(dir.path().join("data.bin"));
        expected.sort();
        assert_eq!(paths, expected);
        assert!(paths.iter().all(|p| temp_path(p).is_file()));
    }
}
