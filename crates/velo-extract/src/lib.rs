//! YouTube extraction: finds the video in an address, asks YouTube's InnerTube API which
//! streams it has, and describes them as [`Format`]s that the engine can download.
//!
//! The extractor asks as several [`Client`]s in turn, since YouTube refuses some clients for
//! some videos; the first that answers with streams wins. It never runs the GUI and never
//! downloads streams itself.

mod client;
mod format;
mod id;
mod player;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reqwest::Url;
use reqwest::header::{CONTENT_TYPE, ORIGIN, USER_AGENT};
use serde::Deserialize;

pub use client::Client;
pub use format::{Format, StreamKind};
pub use id::{is_video_id, video_id};
pub use player::Player;

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
    /// The player script no longer has the shape the extractor looks for.
    #[error("YouTube's player changed and Velo can't read it yet ({0})")]
    PlayerChanged(String),
    /// The player's functions failed when run.
    #[error("YouTube's player script failed: {0}")]
    PlayerFailed(String),
    /// The player script couldn't be cached.
    #[error("couldn't cache YouTube's player: {0}")]
    Cache(#[from] std::io::Error),
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

/// One way to download a video: a single stream, or a video stream and a sound stream that
/// are joined after downloading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    /// One stream: video with sound, or sound only.
    Single(&'a Format),
    /// An MP4 video-only stream and an M4A sound stream, joined into one MP4.
    Joined {
        /// The video.
        video: &'a Format,
        /// The sound.
        audio: &'a Format,
    },
}

impl Choice<'_> {
    /// The itags to download, in order: video then sound when joined.
    pub fn itags(&self) -> Vec<u32> {
        self.formats().iter().map(|f| f.itag).collect()
    }

    /// The streams to download, in order.
    pub fn formats(&self) -> Vec<&Format> {
        match *self {
            Self::Single(format) => vec![format],
            Self::Joined { video, audio } => vec![video, audio],
        }
    }

    /// The total size, if YouTube gave every stream's.
    pub fn size(&self) -> Option<u64> {
        self.formats().iter().map(|f| f.content_length).sum()
    }

    /// The file extension of the result.
    pub fn extension(&self) -> &str {
        match self {
            Self::Single(format) => format.extension(),
            Self::Joined { .. } => "mp4",
        }
    }

    /// The picture height, for video.
    pub fn height(&self) -> Option<u32> {
        self.formats()[0].height
    }

    /// Whether it has sound only.
    pub fn is_audio_only(&self) -> bool {
        matches!(self, Self::Single(f) if f.kind == StreamKind::Audio)
    }
}

impl Video {
    /// The ways worth offering to download it, best first: video (with sound in the stream,
    /// or joined from an MP4 video stream and the best M4A sound) by resolution, frame rate
    /// and bitrate, then sound only by bitrate.
    ///
    /// For each resolution only the best MP4 video stream is joined, H.264 before other
    /// codecs since more players handle it. WebM streams aren't joined.
    pub fn choices(&self) -> Vec<Choice<'_>> {
        let rank = |f: &Format| (f.height, f.fps, f.bitrate);
        let mut audio: Vec<&Format> = self
            .formats
            .iter()
            .filter(|f| f.kind == StreamKind::Audio)
            .collect();
        audio.sort_by_key(|f| std::cmp::Reverse(f.bitrate));
        let best_m4a = audio.iter().copied().find(|f| f.mime_type == "audio/mp4");

        let mut videos: Vec<Choice<'_>> = self
            .formats
            .iter()
            .filter(|f| f.kind == StreamKind::Muxed)
            .map(Choice::Single)
            .collect();
        if let Some(audio) = best_m4a {
            let mut by_height: Vec<&Format> = Vec::new();
            for format in self
                .formats
                .iter()
                .filter(|f| f.kind == StreamKind::Video && f.mime_type == "video/mp4")
            {
                let h264 = |f: &Format| f.codecs.iter().any(|c| c.starts_with("avc1"));
                match by_height.iter_mut().find(|f| f.height == format.height) {
                    Some(best) => {
                        if (h264(format), rank(format)) > (h264(best), rank(best)) {
                            *best = format;
                        }
                    }
                    None => by_height.push(format),
                }
            }
            videos.extend(
                by_height
                    .into_iter()
                    .map(|video| Choice::Joined { video, audio }),
            );
        }
        videos.sort_by_key(|c| std::cmp::Reverse(rank(c.formats()[0])));
        videos
            .into_iter()
            .chain(audio.into_iter().map(Choice::Single))
            .collect()
    }

    /// The choice that downloads `itags`: one stream, or a video and its sound to join.
    pub fn choice(&self, itags: &[u32]) -> Option<Choice<'_>> {
        match itags {
            [itag] => Some(Choice::Single(self.format(*itag)?)),
            [video, audio] => Some(Choice::Joined {
                video: self.format(*video)?,
                audio: self.format(*audio)?,
            }),
            _ => None,
        }
    }

    /// The format with `itag`.
    pub fn format(&self, itag: u32) -> Option<&Format> {
        self.formats.iter().find(|f| f.itag == itag)
    }

    /// A file name for `choice` of this video: the title, made safe, with the result's
    /// extension.
    pub fn file_name(&self, choice: &Choice<'_>) -> String {
        let title: String = self
            .title
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c if c.is_control() => '_',
                c => c,
            })
            .collect();
        let title = title.trim().trim_end_matches('.');
        let title = if title.is_empty() {
            self.id.as_str()
        } else {
            title
        };
        format!("{title}.{}", choice.extension())
    }
}

/// Asks YouTube about videos.
#[derive(Debug, Clone)]
pub struct Extractor {
    http: reqwest::Client,
    base: Url,
    clients: Vec<Client>,
    /// Where player scripts are kept between runs; memory only if `None`.
    cache_dir: Option<PathBuf>,
    /// The player last read, shared by clones.
    player: Arc<Mutex<Option<Arc<Player>>>>,
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
            cache_dir: None,
            player: Arc::default(),
        }
    }

    /// Keeps the player scripts it downloads in `dir`, the newest few, so they needn't be
    /// downloaded again after a restart.
    pub fn with_cache_dir(mut self, dir: PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
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
            let player = if client.needs_player {
                match self.load_player().await {
                    Ok(player) => Some(player),
                    Err(error) => {
                        refusals.push(format!("{}: {error}", client.name));
                        continue;
                    }
                }
            } else {
                None
            };
            let timestamp = player.as_ref().and_then(|p| p.signature_timestamp);
            let response = match self.ask(client, id, timestamp).await {
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
            let mut formats: Vec<Format> = streams
                .formats
                .into_iter()
                .filter_map(|f| f.into_format(false))
                .chain(
                    streams
                        .adaptive_formats
                        .into_iter()
                        .filter_map(|f| f.into_format(true)),
                )
                .collect();
            if let Some(player) = player {
                formats = match decipher(player, formats).await {
                    Ok(formats) => formats,
                    Err(error) => {
                        refusals.push(format!("{}: {error}", client.name));
                        continue;
                    }
                };
            }
            formats.retain(|f| !f.needs_deciphering());
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

    /// Asks `/youtubei/v1/player` about video `id` as `client`, sending the player's
    /// `timestamp` if it has one.
    async fn ask(
        &self,
        client: &Client,
        id: &str,
        timestamp: Option<u64>,
    ) -> Result<PlayerResponse> {
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
            .body(client.player_request(id, timestamp).to_string())
            .send()
            .await?
            .error_for_status()?;
        Ok(serde_json::from_slice(&response.bytes().await?)?)
    }
}

impl Extractor {
    /// The current player, read from the cache if this version was seen before.
    async fn load_player(&self) -> Result<Arc<Player>> {
        let api = self.text("iframe_api").await?;
        let id = player::player_id(&api)
            .ok_or_else(|| Error::PlayerChanged("couldn't find the player version".into()))?;
        if let Some(player) = self.player.lock().unwrap().as_ref()
            && player.id == id
        {
            return Ok(player.clone());
        }
        let cached = self
            .cache_dir
            .as_ref()
            .map(|dir| dir.join(format!("{id}.js")));
        let js = match &cached {
            Some(path) if path.exists() => tokio::fs::read_to_string(path).await?,
            _ => {
                let js = self.text(&player::player_path(&id)).await?;
                if let Some(path) = &cached {
                    self.save_player(path, &js).await?;
                }
                js
            }
        };
        let player = tokio::task::spawn_blocking(move || Player::from_js(&id, &js))
            .await
            .map_err(|e| Error::PlayerFailed(e.to_string()))??;
        let player = Arc::new(player);
        *self.player.lock().unwrap() = Some(player.clone());
        Ok(player)
    }

    /// Writes player script `js` to `path`, keeping only the newest [`CACHED_PLAYERS`].
    async fn save_player(&self, path: &std::path::Path, js: &str) -> Result<()> {
        let dir = path.parent().expect("a file in the cache folder");
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(path, js).await?;
        let mut scripts = Vec::new();
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().is_some_and(|e| e == "js") {
                let modified = entry.metadata().await?.modified()?;
                scripts.push((modified, entry.path()));
            }
        }
        scripts.sort();
        let excess = scripts.len().saturating_sub(CACHED_PLAYERS);
        for (_, old) in scripts.into_iter().take(excess) {
            if old != path {
                tokio::fs::remove_file(old).await?;
            }
        }
        Ok(())
    }

    /// The text at `path` on YouTube.
    async fn text(&self, path: &str) -> Result<String> {
        let url = self.base.join(path).expect("a valid path");
        let response = self
            .http
            .get(url)
            .header(USER_AGENT, Client::WEB.user_agent)
            .send()
            .await?
            .error_for_status()?;
        Ok(response.text().await?)
    }
}

/// How many player scripts the cache keeps.
const CACHED_PLAYERS: usize = 3;

/// Unscrambles the signatures and transforms the `n` parameters of `formats` with `player`.
async fn decipher(player: Arc<Player>, mut formats: Vec<Format>) -> Result<Vec<Format>> {
    // The address of each format with its scrambled signature and the parameter name for it.
    let mut ciphered = Vec::new();
    for (index, format) in formats.iter_mut().enumerate() {
        if let Some(cipher) = format.signature_cipher.take() {
            let pairs = query_pairs(&cipher);
            let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            let (Some(url), Some(signature)) = (get("url"), get("s")) else {
                continue;
            };
            format.url = Some(url);
            let parameter = get("sp").unwrap_or_else(|| "signature".into());
            ciphered.push((index, signature, parameter));
        }
    }
    let ns: Vec<(usize, String)> = formats
        .iter()
        .enumerate()
        .filter_map(|(index, f)| {
            let url = f.url.as_deref()?;
            let n = query_pairs(url.split_once('?')?.1)
                .into_iter()
                .find(|(k, _)| k == "n")?
                .1;
            Some((index, n))
        })
        .collect();
    let signatures: Vec<String> = ciphered.iter().map(|(_, s, _)| s.clone()).collect();
    let n_inputs: Vec<String> = ns.iter().map(|(_, n)| n.clone()).collect();
    let (signatures, n_outputs) =
        tokio::task::spawn_blocking(move || player.solve(&signatures, &n_inputs))
            .await
            .map_err(|e| Error::PlayerFailed(e.to_string()))??;
    for ((index, _, parameter), signature) in ciphered.into_iter().zip(signatures) {
        set_parameter(&mut formats[index], &parameter, &signature);
    }
    for ((index, _), n) in ns.into_iter().zip(n_outputs) {
        set_parameter(&mut formats[index], "n", &n);
    }
    Ok(formats)
}

/// The decoded `key=value` pairs of query string `query`.
fn query_pairs(query: &str) -> Vec<(String, String)> {
    Url::parse(&format!("http://x/?{query}"))
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

/// Sets query parameter `key` of `format`'s address to `value`, replacing an old one.
fn set_parameter(format: &mut Format, key: &str, value: &str) {
    let Some(url) = format.url.as_deref().and_then(|u| Url::parse(u).ok()) else {
        return;
    };
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .into_owned()
        .filter(|(k, _)| k != key)
        .collect();
    let mut url = url;
    url.query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair(key, value);
    format.url = Some(url.into());
}

impl velo_core::Resolver for Extractor {
    /// The current addresses of a YouTube source's formats, in its order.
    fn resolve<'a>(&'a self, source: &'a velo_core::Source) -> velo_core::Resolving<'a> {
        Box::pin(async move {
            let velo_core::Source::YouTube { video_id, itags } = source;
            let video = self.video(video_id).await.map_err(|e| e.to_string())?;
            itags
                .iter()
                .map(|itag| {
                    video
                        .format(*itag)
                        .and_then(|f| f.url.clone())
                        .ok_or_else(|| format!("YouTube no longer offers format {itag}"))
                })
                .collect()
        })
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
