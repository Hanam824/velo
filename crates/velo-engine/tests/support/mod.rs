//! A local HTTP server for the engine's tests, so they never touch the internet.
//!
//! Routes (every file body is [`body`] cut to the given size):
//! - `/files/{name}`: answers `HEAD` and ranges, with `Accept-Ranges`, `ETag` and `Last-Modified`.
//! - `/norange/{name}`: ignores `Range` and always sends the whole file with `200`.
//! - `/nohead/{name}`: rejects `HEAD` with `405` and leaves out `Accept-Ranges`, but answers ranges.
//! - `/attachment`: like `/files`, with a UTF-8 `Content-Disposition` file name.
//! - `/empty.txt`: an empty file.
//! - `/redirect`: redirects to `/files/redirected.bin`.
//! - anything else: `404`.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
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

/// Starts the server on a free local port and returns its base URL, e.g. `http://127.0.0.1:1234`.
pub async fn start() -> String {
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
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
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
