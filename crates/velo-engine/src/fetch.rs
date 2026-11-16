//! Requests over one connection: a whole file into a file, and a byte range.

use std::path::Path;

use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{Client, Response, StatusCode, Url};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

use crate::probe::content_range_start;
use crate::{Error, Limiter, Result};

/// Bytes a download buffers before it hands them to the OS.
pub(crate) const WRITE_BUFFER: usize = 256 << 10;

/// Downloads all of `url` over one connection into `path`, replacing what the file holds, and
/// returns the file's length.
///
/// This is for a server without range support, where the only way to fetch is from the first
/// byte. Every chunk passes each of `limiters` first. `on_progress` is called with the number
/// of bytes written after each chunk. Returns [`Error::Incomplete`] if the connection ends
/// before the server's `Content-Length`, and [`Error::Cancelled`] once `cancel` is cancelled.
pub async fn fetch_whole(
    client: &Client,
    url: &Url,
    path: &Path,
    limiters: &[Limiter],
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64),
) -> Result<u64> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).await?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .await?;
    let mut response = cancellable(cancel, async {
        client
            .get(url.clone())
            .send()
            .await
            .map_err(Error::from)
            .inspect_err(|error| {
                tracing::warn!(url = velo_log::redact::url(url.as_str()), %error, "request failed");
            })
    })
    .await?;
    if !response.status().is_success() {
        tracing::warn!(url = velo_log::redact::url(url.as_str()), status = %response.status(), "request refused");
        return Err(Error::Status(response.status()));
    }
    let expected = response.content_length();

    let mut writer = BufWriter::with_capacity(WRITE_BUFFER, file);
    let mut have = 0;
    while let Some(chunk) = cancellable(cancel, async { Ok(response.chunk().await?) }).await? {
        for limiter in limiters {
            cancellable(cancel, async {
                limiter.acquire(chunk.len() as u64).await;
                Ok(())
            })
            .await?;
        }
        writer.write_all(&chunk).await?;
        have += chunk.len() as u64;
        on_progress(chunk.len() as u64);
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
    let sent = std::time::Instant::now();
    let response = client
        .get(url.clone())
        .header(RANGE, range.clone())
        .send()
        .await
        .map_err(Error::from)
        .inspect_err(|error| {
            // The cause says what "error sending request" hides: a refused or reset connection,
            // a timeout, a TLS failure.
            let causes: Vec<String> =
                std::iter::successors(std::error::Error::source(error), |e| e.source())
                    .skip(1)
                    .map(ToString::to_string)
                    .collect();
            tracing::warn!(
                url = velo_log::redact::url(url.as_str()),
                range,
                %error,
                cause = causes.join(": "),
                after = ?sent.elapsed(),
                "range request failed"
            );
        })?;
    let status = response.status();
    if status.is_success() {
        // `after` is the connection (a new one per request) plus the server's time to answer.
        tracing::debug!(url = velo_log::redact::url(url.as_str()), range, %status, after = ?sent.elapsed(), "range answered");
    } else {
        tracing::warn!(
            url = velo_log::redact::url(url.as_str()),
            range,
            %status,
            content_type = ?response.headers().get(reqwest::header::CONTENT_TYPE),
            server = ?response.headers().get(reqwest::header::SERVER),
            "range refused"
        );
    }
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
