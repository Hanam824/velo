//! Downloads one byte range over one connection into a part file.

use std::io::SeekFrom;
use std::path::Path;

use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use reqwest::{Client, Response, StatusCode, Url};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

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
/// [`Error::Incomplete`] if the connection ends early, and [`Error::Cancelled`] once `cancel` is
/// cancelled; the bytes received so far stay in the part file for the next attempt.
pub async fn fetch_range(
    client: &Client,
    url: &Url,
    part_path: &Path,
    start: u64,
    end: Option<u64>,
    cancel: &CancellationToken,
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
    let mut response = if position > 0 || end.is_some() {
        cancellable(cancel, send_range(client, url, position, end)).await?
    } else {
        let response =
            cancellable(cancel, async { Ok(client.get(url.clone()).send().await?) }).await?;
        if !response.status().is_success() {
            return Err(Error::Status(response.status()));
        }
        response
    };
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
    loop {
        let chunk = match cancellable(cancel, async { Ok(response.chunk().await?) }).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                writer.flush().await?;
                return Err(error);
            }
        };
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

/// Runs `future`, or returns [`Error::Cancelled`] as soon as `cancel` is cancelled.
pub(crate) async fn cancellable<T>(
    cancel: &CancellationToken,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(Error::Cancelled),
        result = future => result,
    }
}

/// Sends `GET` with `Range: bytes=<position>-[<end - 1>]` and checks that the answer is a `206`
/// starting at `position`.
pub(crate) async fn send_range(
    client: &Client,
    url: &Url,
    position: u64,
    end: Option<u64>,
) -> Result<Response> {
    let range = match end {
        Some(end) => format!("bytes={position}-{}", end - 1),
        None => format!("bytes={position}-"),
    };
    let response = client.get(url.clone()).header(RANGE, range).send().await?;
    let status = response.status();
    let range_start = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(content_range_start);
    if status == StatusCode::PARTIAL_CONTENT && range_start == Some(position) {
        Ok(response)
    } else if status.is_success() {
        Err(Error::RangeIgnored)
    } else {
        Err(Error::Status(status))
    }
}
