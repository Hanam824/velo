//! Asks a server about a URL before downloading: size, range support, file name and version.

use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG, HeaderMap,
    LAST_MODIFIED, RANGE,
};
use reqwest::{Client, StatusCode, Url};

use crate::{Error, Result, file_name};

/// What the server said about a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The URL after redirects; requests for the file's bytes go here.
    pub url: Url,
    /// Total size in bytes, if the server sent one.
    pub total_size: Option<u64>,
    /// Whether the server answers range requests and sent a size, so the download can be split
    /// and resumed.
    pub resumable: bool,
    /// File name from `Content-Disposition` or the URL, already sanitized.
    pub file_name: String,
    /// The server's `ETag`.
    pub etag: Option<String>,
    /// The server's `Last-Modified`.
    pub last_modified: Option<String>,
}

/// Probes `url` with a `HEAD` request, falling back to `GET` with `Range: bytes=0-0`.
///
/// The fallback runs when `HEAD` fails, or doesn't give both a size and `Accept-Ranges: bytes`:
/// many servers reject `HEAD` or leave out `Accept-Ranges` but still answer ranges. A `206`
/// answer means the server supports ranges; a `200` means it doesn't, and its body is dropped
/// unread.
pub async fn probe(client: &Client, url: &str) -> Result<Probe> {
    let head = match client.head(url).send().await {
        Ok(response) if response.status().is_success() => Some(response),
        _ => None,
    };
    if let Some(head) = &head {
        let size = content_length(head.headers());
        let ranges = head
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
        // An empty file has no range to ask for.
        if size == Some(0) || (size.is_some() && ranges) {
            return Ok(from_headers(
                head.url(),
                head.headers(),
                None,
                size,
                size != Some(0),
            ));
        }
    }

    let response = client.get(url).header(RANGE, "bytes=0-0").send().await?;
    let head_headers = head.as_ref().map(|h| h.headers());
    match response.status() {
        StatusCode::PARTIAL_CONTENT => {
            let total = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(content_range_total);
            Ok(from_headers(
                response.url(),
                response.headers(),
                head_headers,
                total,
                total.is_some(),
            ))
        }
        status if status.is_success() => {
            let size = content_length(response.headers());
            Ok(from_headers(
                response.url(),
                response.headers(),
                head_headers,
                size,
                false,
            ))
        }
        status => Err(Error::Status(status)),
    }
}

/// Builds a [`Probe`] from a response's headers, taking any missing header from `fallback`
/// (the `HEAD` response, when the `GET` fallback ran).
fn from_headers(
    url: &Url,
    headers: &HeaderMap,
    fallback: Option<&HeaderMap>,
    total_size: Option<u64>,
    resumable: bool,
) -> Probe {
    let header = |name| {
        let get = |h: &HeaderMap| h.get(&name).and_then(|v| v.to_str().ok()).map(String::from);
        get(headers).or_else(|| fallback.and_then(get))
    };
    Probe {
        url: url.clone(),
        total_size,
        resumable,
        file_name: file_name(header(CONTENT_DISPOSITION).as_deref(), url),
        etag: header(ETAG),
        last_modified: header(LAST_MODIFIED),
    }
}

/// The `Content-Length` header as a number.
fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The total size in a `Content-Range: bytes <first>-<last>/<total>` value; `None` if the total
/// is `*` or the value is malformed.
fn content_range_total(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split_once('/')?
        .1
        .trim()
        .parse()
        .ok()
}

/// The first byte offset in a `Content-Range: bytes <first>-<last>/<total>` value.
pub(crate) fn content_range_start(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split_once('-')?
        .0
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_values_are_parsed() {
        assert_eq!(content_range_total("bytes 0-0/12345"), Some(12345));
        assert_eq!(content_range_total("bytes 0-0/*"), None);
        assert_eq!(content_range_total("items 0-0/5"), None);
        assert_eq!(content_range_start("bytes 100-199/1000"), Some(100));
    }
}
