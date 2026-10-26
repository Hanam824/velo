//! A local HTTP server for the engine's tests, so they never touch the internet.
//!
//! Routes (every file body is [`body`] cut to the given size):
//! - `/files/{name}`: answers `HEAD` and ranges, with `Accept-Ranges`, `ETag` and `Last-Modified`.
//! - `/norange/{name}`: ignores `Range` and always sends the whole file with `200`.
//! - `/nohead/{name}`: rejects `HEAD` with `405` and leaves out `Accept-Ranges`, but answers ranges.
//! - `/attachment`: like `/files`, with a UTF-8 `Content-Disposition` file name.
//! - `/empty.txt`: an empty file.
//! - `/redirect`: redirects to `/files/redirected.bin`.
//! - `/slow/{name}`: like `/files`, sent in 8 KiB chunks 2 ms apart, so connections overlap.
//! - `/flaky/{name}`: like `/slow`, but every other `GET` stops halfway through its range.
//! - `/fail/{name}`: answers `HEAD` like `/files`, but every `GET` with `503`.
//! - `/slower/{name}`: like `/slow`, in 2 KiB chunks 10 ms apart: a file takes several of the
//!   engine's 250 ms progress intervals, so a test can pause or delete it mid-way.
//! - `/versioned/{name}`: like `/slower`, with [`versioned_body`] and `ETag` `"v<n>"` for the
//!   current version `n`, which starts at 0.
//! - `/bump`: moves `/versioned` to the next version.
//! - anything else: `404`.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG, LAST_MODIFIED,
    LOCATION, RANGE,
};
use axum::http::{HeaderMap, Method, Response, StatusCode};
use axum::routing::get;

/// Size of the files the server sends.
pub const SIZE: usize = 300_000;

/// `ETag` of the files under `/files`.
pub const ETAG_VALUE: &str = "\"v1\"";

/// `Last-Modified` of the files under `/files`.
pub const LAST_MODIFIED_VALUE: &str = "Mon, 26 Oct 2026 00:00:00 GMT";

/// The bytes of every file: a fixed pattern that isn't periodic over short distances, so a
/// misplaced range shows up as a mismatch.
pub fn body() -> Vec<u8> {
    (0..SIZE as u64)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

/// The bytes of version `n` of `/versioned`: [`body`] with every byte XORed with `n`.
pub fn versioned_body(n: u8) -> Vec<u8> {
    body().into_iter().map(|b| b ^ n).collect()
}

/// How fast [`trickle`] sends: chunk size and the pause before each chunk.
#[derive(Clone, Copy)]
struct Pace(usize, Duration);

/// 8 KiB every 2 ms.
const SLOW: Pace = Pace(8 * 1024, Duration::from_millis(2));

/// 2 KiB every 10 ms.
const SLOWER: Pace = Pace(2 * 1024, Duration::from_millis(10));

/// Starts the server on a free local port and returns its base URL, e.g. `http://127.0.0.1:1234`.
pub async fn start() -> String {
    let gets = Arc::new(AtomicUsize::new(0));
    let version = Arc::new(AtomicUsize::new(0));
    let bump = Arc::clone(&version);
    let app = Router::new()
        .route(
            "/files/{name}",
            get(|method: Method, headers: HeaderMap| async move {
                serve(&method, &headers, &body(), true, &[])
            }),
        )
        .route(
            "/norange/{name}",
            get(|method: Method, headers: HeaderMap| async move {
                serve(&method, &headers, &body(), false, &[])
            }),
        )
        .route(
            "/nohead/{name}",
            get(|method: Method, headers: HeaderMap| async move {
                if method == Method::HEAD {
                    return status(StatusCode::METHOD_NOT_ALLOWED);
                }
                let mut response = serve(&method, &headers, &body(), true, &[]);
                response.headers_mut().remove(ACCEPT_RANGES);
                response
            }),
        )
        .route(
            "/attachment",
            get(|method: Method, headers: HeaderMap| async move {
                let disposition =
                    "attachment; filename=\"resume.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf";
                serve(
                    &method,
                    &headers,
                    &body(),
                    true,
                    &[(CONTENT_DISPOSITION, disposition)],
                )
            }),
        )
        .route(
            "/empty.txt",
            get(|method: Method, headers: HeaderMap| async move {
                serve(&method, &headers, &[], true, &[])
            }),
        )
        .route(
            "/redirect",
            get(|| async {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(LOCATION, "/files/redirected.bin")
                    .body(Body::empty())
                    .unwrap()
            }),
        )
        .route(
            "/slow/{name}",
            get(|method: Method, headers: HeaderMap| async move {
                trickle(serve(&method, &headers, &body(), true, &[]), false, SLOW).await
            }),
        )
        .route(
            "/flaky/{name}",
            get(move |method: Method, headers: HeaderMap| {
                let gets = Arc::clone(&gets);
                async move {
                    let response = serve(&method, &headers, &body(), true, &[]);
                    let cut = method == Method::GET
                        && gets.fetch_add(1, Ordering::Relaxed).is_multiple_of(2);
                    trickle(response, cut, SLOW).await
                }
            }),
        )
        .route(
            "/slower/{name}",
            get(|method: Method, headers: HeaderMap| async move {
                trickle(serve(&method, &headers, &body(), true, &[]), false, SLOWER).await
            }),
        )
        .route(
            "/versioned/{name}",
            get(move |method: Method, headers: HeaderMap| {
                let n = version.load(Ordering::Relaxed) as u8;
                async move {
                    let mut response = serve(&method, &headers, &versioned_body(n), true, &[]);
                    let etag = format!("\"v{n}\"").parse().unwrap();
                    response.headers_mut().insert(ETAG, etag);
                    trickle(response, false, SLOWER).await
                }
            }),
        )
        .route(
            "/bump",
            get(move || async move {
                bump.fetch_add(1, Ordering::Relaxed);
                status(StatusCode::NO_CONTENT)
            }),
        )
        .route(
            "/fail/{name}",
            get(|method: Method, headers: HeaderMap| async move {
                if method == Method::GET {
                    return status(StatusCode::SERVICE_UNAVAILABLE);
                }
                serve(&method, &headers, &body(), true, &[])
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

/// Re-sends `response`'s body at `pace`; with `cut`, the connection fails halfway through.
async fn trickle(response: Response<Body>, cut: bool, pace: Pace) -> Response<Body> {
    let Pace(chunk, delay) = pace;
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let stop = if cut { bytes.len() / 2 } else { bytes.len() };
    let chunks = futures_util::stream::unfold(0, move |sent| {
        let bytes = bytes.clone();
        async move {
            if sent >= stop {
                // After the error the stream ends; `usize::MAX` marks that.
                return (cut && sent != usize::MAX).then(|| {
                    let error = std::io::Error::other("connection cut by the test server");
                    (Err(error), usize::MAX)
                });
            }
            tokio::time::sleep(delay).await;
            let next = (sent + chunk).min(stop);
            Some((Ok::<Bytes, std::io::Error>(bytes.slice(sent..next)), next))
        }
    });
    Response::from_parts(parts, Body::from_stream(chunks))
}

/// An empty response with `code`.
fn status(code: StatusCode) -> Response<Body> {
    Response::builder()
        .status(code)
        .body(Body::empty())
        .unwrap()
}

/// Sends `data`, or the part of it that a `Range: bytes=<first>-[<last>]` header asks for when
/// `ranges` is set.
fn serve(
    method: &Method,
    headers: &HeaderMap,
    data: &[u8],
    ranges: bool,
    extra: &[(axum::http::HeaderName, &str)],
) -> Response<Body> {
    let len = data.len() as u64;
    let mut builder = Response::builder();
    if ranges {
        builder = builder
            .header(ACCEPT_RANGES, "bytes")
            .header(ETAG, ETAG_VALUE)
            .header(LAST_MODIFIED, LAST_MODIFIED_VALUE);
    }
    for (name, value) in extra {
        builder = builder.header(name, *value);
    }

    let range = headers
        .get(RANGE)
        .filter(|_| ranges)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .and_then(|(first, last)| {
            let first: u64 = first.parse().ok()?;
            let last: u64 = if last.is_empty() {
                len.checked_sub(1)?
            } else {
                last.parse().ok()?
            };
            Some((first, last.min(len.saturating_sub(1))))
        });
    let (code, first, last) = match (headers.get(RANGE).filter(|_| ranges), range) {
        (None, _) => (StatusCode::OK, 0, len),
        (Some(_), Some((first, last))) if first <= last && first < len => {
            builder = builder.header(CONTENT_RANGE, format!("bytes {first}-{last}/{len}"));
            (StatusCode::PARTIAL_CONTENT, first, last + 1)
        }
        (Some(_), _) => {
            return builder
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .unwrap();
        }
    };
    let slice = data[first as usize..last as usize].to_vec();
    builder = builder.status(code).header(CONTENT_LENGTH, slice.len());
    let body = if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(slice)
    };
    builder.body(body).unwrap()
}
