//! Downloads a URL from start to finish: probe, fetch, merge.

use std::path::{Path, PathBuf};

use reqwest::Client;
use tokio::fs;
use velo_core::{DownloadId, Segment, SegmentId, SegmentState};

use crate::{Options, Progress, Result, download_segments, fetch_range, merge, planner, probe};

/// Downloads `url` into `save_dir` and returns the path of the finished file.
///
/// The file is named from the server's answer (see [`file_name`](crate::file_name)), and its
/// parts go in `<temp_dir>/<id>/`. A server that answers range requests gets up to
/// `options.max_connections` connections and resumes from any part files already there. One that
/// doesn't gets a single connection, starting over from the first byte. `progress` is filled with
/// the download's segments once the server has answered, so a caller can watch it.
pub async fn download_file(
    client: &Client,
    url: &str,
    temp_dir: &Path,
    id: DownloadId,
    save_dir: &Path,
    options: &Options,
    progress: &Progress,
) -> Result<PathBuf> {
    let probe = probe(client, url).await?;
    let dir = temp_dir.join(id.0.to_string());
    let save_path = save_dir.join(&probe.file_name);

    if probe.resumable {
        let total = probe.total_size.unwrap_or_default();
        progress.replace(planner::initial_segments(total));
        download_segments(client, &probe.url, &dir, progress, options).await?;
    } else {
        // Without ranges the only way to fetch is from the first byte.
        let segment = Segment::new(SegmentId(0), 0, 0);
        let part = dir.join(segment.part_file_name());
        if fs::try_exists(&part).await? {
            fs::remove_file(&part).await?;
        }
        progress.replace(vec![segment]);
        let len = fetch_range(client, &probe.url, &part, 0, None, |n| {
            progress.update(SegmentId(0), |s| {
                s.end += n;
                s.downloaded += n;
            })
        })
        .await?;
        progress.update(SegmentId(0), |s| {
            (s.end, s.downloaded, s.state) = (len, len, SegmentState::Done)
        });
    }

    merge(&dir, &progress.snapshot(), probe.total_size, &save_path).await?;
    Ok(save_path)
}
