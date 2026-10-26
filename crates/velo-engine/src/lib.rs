//! The segmented download engine: probes a URL, splits it into ranges, downloads each range into a part file and merges the parts.

use std::path::{Path, PathBuf};
use std::sync::Once;

use velo_core::{DownloadId, ProxyMode, Segment, Settings};

mod download;
mod engine;
mod fetch;
mod filename;
mod limiter;
mod merge;
pub mod planner;
mod probe;
mod segmented;

pub use download::download_file;
pub use engine::{Engine, EngineConfig, NewDownload, RetryPolicy};
pub use fetch::fetch_range;
pub use filename::{file_name, sanitize_file_name, unique_file_name};
pub use limiter::Limiter;
pub use merge::merge;
pub use probe::{Probe, probe};
pub use reqwest::Url;
pub use segmented::{Options, Progress, download_segments};
pub use tokio_util::sync::CancellationToken;

/// Errors from the download engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The HTTP request failed: bad URL, connection, TLS or protocol error.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Reading or writing a part file failed.
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    /// The server answered with an error status.
    #[error("server answered {0}")]
    Status(reqwest::StatusCode),
    /// The server didn't honour a range request, or answered with a different range.
    #[error("server ignored the range request")]
    RangeIgnored,
    /// The connection ended before the expected number of bytes arrived.
    #[error("connection closed after {received} of {expected} bytes")]
    Incomplete {
        /// Bytes that arrived.
        received: u64,
        /// Bytes that were expected.
        expected: u64,
    },
    /// A merge was asked for before every segment was done, or the segments don't cover the
    /// whole file.
    #[error("the download isn't finished")]
    NotFinished,
    /// A part file's size doesn't match its segment.
    #[error("part file {path} holds {actual} bytes, expected {expected}")]
    PartSize {
        /// The part file.
        path: PathBuf,
        /// The segment's length.
        expected: u64,
        /// The part file's size.
        actual: u64,
    },
    /// The merged file's size doesn't match the server's.
    #[error("merged file holds {actual} bytes, expected {expected}")]
    SizeMismatch {
        /// Size from the server.
        expected: u64,
        /// Size of the merged file.
        actual: u64,
    },
    /// The download was stopped by its caller.
    #[error("the download was stopped")]
    Cancelled,
    /// A referer or cookie value can't be sent as an HTTP header.
    #[error("a header value contains characters HTTP doesn't allow")]
    InvalidHeader,
    /// Something already exists at the save path.
    #[error("{0} already exists")]
    Exists(PathBuf),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Builds the HTTP client the engine shares between requests, so connections to a server are
/// reused, with the default settings.
///
/// It verifies certificates against the operating system's trust store, follows redirects and
/// identifies itself as `Velo/<version>`. Responses are never decompressed, so byte offsets
/// match the file on the server.
pub fn client() -> Result<reqwest::Client> {
    client_with(&Settings::default(), &RequestOptions::default())
}

/// Builds a client like [`client`] that uses the user agent and proxy from `settings` and
/// sends `request`'s headers with every request.
pub fn client_with(settings: &Settings, request: &RequestOptions) -> Result<reqwest::Client> {
    static PROVIDER: Once = Once::new();
    PROVIDER.call_once(|| {
        // Fails only if another provider is already installed, which is fine too.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    let mut builder = reqwest::Client::builder()
        .user_agent(
            settings
                .user_agent
                .clone()
                .unwrap_or_else(|| concat!("Velo/", env!("CARGO_PKG_VERSION")).into()),
        )
        .default_headers(request.headers()?);
    builder = match settings.proxy.mode {
        ProxyMode::None => builder.no_proxy(),
        ProxyMode::System => builder,
        ProxyMode::Manual => builder.proxy(reqwest::Proxy::all(&settings.proxy.url)?),
    };
    Ok(builder.build()?)
}

/// Headers a download sends with every request besides the defaults.
///
/// The engine keeps these in memory only; they are never written to the download list, since
/// credentials and cookies are secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestOptions {
    /// Sent as HTTP Basic authorization.
    pub credentials: Option<Credentials>,
    /// The page the link came from, sent as `Referer`.
    pub referer: Option<String>,
    /// Sent as the `Cookie` header, such as `a=1; b=2`.
    pub cookies: Option<String>,
}

impl RequestOptions {
    /// Whether no header is set, so the shared client will do.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The headers to send; secrets are marked sensitive so they never show in logs.
    fn headers(&self) -> Result<reqwest::header::HeaderMap> {
        use reqwest::header::{AUTHORIZATION, COOKIE, HeaderValue, REFERER};
        let mut headers = reqwest::header::HeaderMap::new();
        let value = |text: &str| HeaderValue::from_str(text).map_err(|_| Error::InvalidHeader);
        if let Some(credentials) = &self.credentials {
            let mut header = credentials.basic_header();
            header.set_sensitive(true);
            headers.insert(AUTHORIZATION, header);
        }
        if let Some(referer) = &self.referer {
            headers.insert(REFERER, value(referer)?);
        }
        if let Some(cookies) = &self.cookies {
            let mut header = value(cookies)?;
            header.set_sensitive(true);
            headers.insert(COOKIE, header);
        }
        Ok(headers)
    }
}

/// A user name and password for a server that asks for them.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The user name.
    pub user: String,
    /// The password.
    pub password: String,
}

impl Credentials {
    /// The `Authorization: Basic …` header value.
    fn basic_header(&self) -> reqwest::header::HeaderValue {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.user, self.password));
        reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
            .expect("base64 is a valid header value")
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &"…")
            .finish()
    }
}

/// Path of a segment's part file: `<temp_dir>/<download id>/<segment id>.part`.
pub fn part_path(temp_dir: &Path, download: DownloadId, segment: &Segment) -> PathBuf {
    temp_dir
        .join(download.0.to_string())
        .join(segment.part_file_name())
}

#[cfg(test)]
mod tests {
    use velo_core::ProxySettings;

    use super::*;

    #[test]
    fn clients_build_for_every_proxy_mode_and_reject_a_bad_proxy() {
        for (mode, url) in [
            (ProxyMode::None, ""),
            (ProxyMode::System, ""),
            (ProxyMode::Manual, "http://127.0.0.1:3128"),
            (ProxyMode::Manual, "socks5://127.0.0.1:1080"),
        ] {
            let settings = Settings {
                proxy: ProxySettings {
                    mode,
                    url: url.into(),
                },
                user_agent: Some("Test/1".into()),
                ..Settings::default()
            };
            assert!(
                client_with(&settings, &RequestOptions::default()).is_ok(),
                "{mode:?} {url}"
            );
        }
        let bad = Settings {
            proxy: ProxySettings {
                mode: ProxyMode::Manual,
                url: "not a proxy".into(),
            },
            ..Settings::default()
        };
        assert!(client_with(&bad, &RequestOptions::default()).is_err());
    }

    #[test]
    fn request_headers_are_checked_and_secrets_hidden() {
        let request = RequestOptions {
            credentials: Some(Credentials {
                user: "me".into(),
                password: "pw".into(),
            }),
            referer: Some("https://example.test/".into()),
            cookies: Some("a=1".into()),
        };
        let headers = request.headers().unwrap();
        assert_eq!(headers.len(), 3);
        assert!(headers[reqwest::header::AUTHORIZATION].is_sensitive());
        assert!(headers[reqwest::header::COOKIE].is_sensitive());
        assert!(!format!("{request:?}").contains("pw"));
        let bad = RequestOptions {
            referer: Some("line\nbreak".into()),
            ..RequestOptions::default()
        };
        assert!(matches!(bad.headers(), Err(Error::InvalidHeader)));
    }
}
