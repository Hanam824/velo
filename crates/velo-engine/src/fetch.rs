//! Downloads one byte range over one connection into a part file.

use std::io::SeekFrom;
use std::path::Path;

use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use reqwest::{Client, StatusCode, Url};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};

use crate::probe::content_range_start;
use crate::{Error, Result};

/// Downloads bytes `[start, end)` of `url` into `part_path`, appending after the bytes the part
/// file already holds, and returns the part file's final length.
///
/// The part file's size on disk is the source of truth for progress: a part file holding `n`
/// bytes resumes at `start + n`, and one already longer than the range is cut back to it. With
/// `end` set to `None` the download runs to the end of the file. A request that doesn't start at
/// offset 0, or has an `end`, is a range request and needs a matching `206` answer; for a server
/// without range support, pass `start` 0, `end` `None` and an empty part file.
///
/// `on_progress` is called with the number of bytes written after each chunk. Returns
/// [`Error::Incomplete`] if the connection ends early; the bytes received so far stay in the part
/// file for the next attempt.
pub async fn fetch_range(
    client: &Client,
    url: &Url,
    part_path: &Path,
    start: u64,
    end: Option<u64>,
    mut on_progress: impl FnMut(u64),
) -> Result<u64> {
    if let Some(dir) = part_path.parent() {
        fs::create_dir_all(dir).await?;
    }
    // Not append mode: Windows can't truncate a file opened for appending.
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(part_path)
        .await?;
    let mut have = file.metadata().await?.len();
    if let Some(end) = end {
        let len = end.saturating_sub(start);
        if have >= len {
            file.set_len(len).await?;
            return Ok(len);
        }
    }
    file.seek(SeekFrom::Start(have)).await?;

    let position = start + have;
    let mut request = client.get(url.clone());
    let ranged = position > 0 || end.is_some();
    if ranged {
        let range = match end {
            Some(end) => format!("bytes={position}-{}", end - 1),
            None => format!("bytes={position}-"),
        };
        request = request.header(RANGE, range);
    }
    let mut response = request.send().await?;
    let status = response.status();
    if ranged {
        let range_start = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_start);
        if status != StatusCode::PARTIAL_CONTENT || range_start != Some(position) {
            return Err(if status.is_success() {
                Error::RangeIgnored
            } else {
                Error::Status(status)
            });
        }
    } else if !status.is_success() {
        return Err(Error::Status(status));
    }
    let expected = match end {
        Some(end) => Some(end - start),
        None => response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(|len| have + len),
    };

    let mut writer = BufWriter::new(file);
    while let Some(chunk) = response.chunk().await? {
        // A server may send more than asked for; keep only the requested range.
        let take = match expected {
            Some(expected) if end.is_some() => chunk.len().min((expected - have) as usize),
            _ => chunk.len(),
        };
        writer.write_all(&chunk[..take]).await?;
        have += take as u64;
        on_progress(take as u64);
        if end.is_some() && Some(have) == expected {
            break;
        }
    }
    writer.flush().await?;

    match expected {
        Some(expected) if have != expected => Err(Error::Incomplete {
            received: have,
            expected,
        }),
        _ => Ok(have),
    }
}
