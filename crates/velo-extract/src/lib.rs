//! YouTube extraction: finds the video in an address, asks YouTube's InnerTube API which
//! streams it has, and describes them as [`Format`]s that the engine can download.
//!
//! The extractor asks as several [`Client`]s in turn, since YouTube refuses some clients for
//! some videos; the first that answers with streams wins. It never runs the GUI and never
//! downloads streams itself.

mod client;
mod format;
mod id;

use reqwest::Url;
use reqwest::header::{CONTENT_TYPE, ORIGIN, USER_AGENT};
use serde::Deserialize;

pub use client::Client;
pub use format::{Format, StreamKind};
pub use id::{is_video_id, video_id};

use crate::format::RawFormat;

/// Errors from asking YouTube about a video.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The address isn't a YouTube video address.
    #[error("not a YouTube video address: {0}")]
    NotYouTube(String),
    /// YouTube says the video doesn't exist or was removed.
    #[error("the video is unavailable: {0}")]
    Unavailable(String),
    /// The video is private.
    #[error("the video is private")]
    Private,
    /// The video is age-restricted and no client could play it without signing in.
    #[error("the video is age-restricted, and YouTube wants a signed-in viewer for it")]
    AgeRestricted,
    /// YouTube wants a signed-in viewer, such as to check for bots.
    #[error("YouTube wants a signed-in viewer: {0}")]
    LoginRequired(String),
    /// No client got streams; says what each one was told.
    #[error("YouTube didn't offer the video's streams: {0}")]
    AllClientsFailed(String),
    /// The request couldn't be made.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// YouTube's answer wasn't what the extractor expects.
    #[error("unreadable answer from YouTube: {0}")]
    Unreadable(#[from] serde_json::Error),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A video and its streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Video {
    /// The 11-character video ID.
    pub id: String,
    /// The title.
    pub title: String,
    /// The channel's name.
    pub author: String,
    /// The length in seconds, if known (live streams have none).
    pub duration: Option<u64>,
    /// The streams, in the order YouTube listed them: progressive first, then adaptive.
    pub formats: Vec<Format>,
    /// The [`Client::name`] that got them.
    pub client: &'static str,
}

/// Asks YouTube about videos.
#[derive(Debug, Clone)]
pub struct Extractor {
    http: reqwest::Client,
    base: Url,
    clients: Vec<Client>,
}

impl Extractor {
    /// The address YouTube's API lives at.
    pub const YOUTUBE: &str = "https://www.youtube.com/";

    /// An extractor that sends its requests through `http` (with the user's proxy) to
    /// YouTube, as the clients in [`Client::DEFAULT_ORDER`].
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            base: Url::parse(Self::YOUTUBE).expect("a valid address"),
            clients: Client::DEFAULT_ORDER.to_vec(),
        }
    }

    /// Sends requests to `base` instead of YouTube; for tests against a local server.
    pub fn with_base(mut self, base: Url) -> Self {
        self.base = base;
        self
    }

    /// Asks as `clients`, in order, instead of the default ones.
    pub fn with_clients(mut self, clients: Vec<Client>) -> Self {
        self.clients = clients;
        self
    }

    /// The video at YouTube address `url`.
    pub async fn video_at(&self, url: &str) -> Result<Video> {
        let id = video_id(url).ok_or_else(|| Error::NotYouTube(url.to_string()))?;
        self.video(&id).await
    }

    /// The video with ID `id`, from the first client that gets its streams.
    ///
    /// Stops at once if YouTube says the video doesn't exist or is private; otherwise tries
    /// every client and reports the most telling refusal.
    pub async fn video(&self, id: &str) -> Result<Video> {
        let mut refusals = Vec::new();
        let mut age_restricted = false;
        let mut login_required = None;
        for client in &self.clients {
            let response = match self.player(client, id).await {
                Ok(response) => response,
                Err(error) => {
                    refusals.push(format!("{}: {error}", client.name));
                    continue;
                }
            };
            let playability = response.playability_status.unwrap_or_default();
            let reason = playability.reason.unwrap_or_default();
            match playability.status.as_str() {
                "OK" => {}
                "ERROR" => return Err(Error::Unavailable(reason)),
                "LOGIN_REQUIRED" if reason.to_lowercase().contains("private") => {
                    return Err(Error::Private);
                }
                "LOGIN_REQUIRED" | "AGE_CHECK_REQUIRED" | "AGE_VERIFICATION_REQUIRED" => {
                    let lower = reason.to_lowercase();
                    if lower.contains("age") || lower.contains("inappropriate") {
                        age_restricted = true;
                    } else {
                        login_required = Some(reason.clone());
                    }
                    refusals.push(format!("{}: {reason}", client.name));
                    continue;
                }
                status => {
                    refusals.push(format!("{}: {status} {reason}", client.name));
                    continue;
                }
            }
            let streams = response.streaming_data.unwrap_or_default();
            let formats: Vec<Format> = streams
                .formats
                .into_iter()
                .filter_map(|f| f.into_format(false))
                .chain(
                    streams
                        .adaptive_formats
                        .into_iter()
                        .filter_map(|f| f.into_format(true)),
                )
                .filter(|f| !f.needs_deciphering())
                .collect();
            if formats.is_empty() {
                refusals.push(format!("{}: no streams", client.name));
                continue;
            }
            let details = response.video_details.unwrap_or_default();
            return Ok(Video {
                id: id.to_string(),
                title: details.title,
                author: details.author,
                duration: details.length_seconds.and_then(|s| s.parse().ok()),
                formats,
                client: client.name,
            });
        }
        Err(if age_restricted {
            Error::AgeRestricted
        } else if let Some(reason) = login_required {
            Error::LoginRequired(reason)
        } else {
            Error::AllClientsFailed(refusals.join("; "))
        })
    }

    /// Asks `/youtubei/v1/player` about video `id` as `client`.
    async fn player(&self, client: &Client, id: &str) -> Result<PlayerResponse> {
        let url = self
            .base
            .join("youtubei/v1/player?prettyPrint=false")
            .expect("a valid path");
        let response = self
            .http
            .post(url)
            .header(USER_AGENT, client.user_agent)
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, "https://www.youtube.com")
            .header("X-YouTube-Client-Name", client.client_id.to_string())
            .header("X-YouTube-Client-Version", client.client_version)
            .body(client.player_request(id, None).to_string())
            .send()
            .await?
            .error_for_status()?;
        Ok(serde_json::from_slice(&response.bytes().await?)?)
    }
}

/// The parts of a `/youtubei/v1/player` answer the extractor reads.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayerResponse {
    playability_status: Option<Playability>,
    video_details: Option<VideoDetails>,
    streaming_data: Option<StreamingData>,
}

/// Whether YouTube plays the video for this client, and why not.
#[derive(Debug, Default, Deserialize)]
struct Playability {
    #[serde(default)]
    status: String,
    reason: Option<String>,
}

/// The video's title and such.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetails {
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    length_seconds: Option<String>,
}

/// The video's streams.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamingData {
    #[serde(default)]
    formats: Vec<RawFormat>,
    #[serde(default)]
    adaptive_formats: Vec<RawFormat>,
}
