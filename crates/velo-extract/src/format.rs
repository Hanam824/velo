//! The streams YouTube offers for a video, read from a player response.

use serde::Deserialize;

/// What a stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamKind {
    /// Video and audio in one file (a "progressive" format).
    Muxed,
    /// Video only (an adaptive format).
    Video,
    /// Audio only (an adaptive format).
    Audio,
}

/// One stream of a video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    /// YouTube's number for this format, stable across videos.
    pub itag: u32,
    /// The stream's address, if YouTube gave it directly.
    pub url: Option<String>,
    /// The scrambled address and signature (`s`, `sp`, `url` as a query string), if the
    /// address needs the player's JavaScript first.
    pub signature_cipher: Option<String>,
    /// The media type without codecs, such as `video/mp4`.
    pub mime_type: String,
    /// The codecs, such as `["avc1.42001E", "mp4a.40.2"]`.
    pub codecs: Vec<String>,
    /// What it carries.
    pub kind: StreamKind,
    /// Frame width in pixels, for video.
    pub width: Option<u32>,
    /// Frame height in pixels, for video.
    pub height: Option<u32>,
    /// Frames per second, for video.
    pub fps: Option<u32>,
    /// Average bits per second.
    pub bitrate: Option<u64>,
    /// Size in bytes, if YouTube says.
    pub content_length: Option<u64>,
    /// YouTube's label, such as `720p60`.
    pub quality_label: Option<String>,
    /// Audio sample rate in Hz, for audio.
    pub audio_sample_rate: Option<u32>,
}

impl Format {
    /// The file extension for this stream: `mp4`, `m4a`, `webm` or `3gp`.
    pub fn extension(&self) -> &str {
        match (self.mime_type.as_str(), self.kind) {
            ("audio/mp4", _) => "m4a",
            ("video/mp4", _) => "mp4",
            ("audio/webm" | "video/webm", _) => "webm",
            ("video/3gpp", _) => "3gp",
            (_, StreamKind::Audio) => "audio",
            _ => "video",
        }
    }

    /// Whether the address needs the player's JavaScript before it can be downloaded.
    pub fn needs_deciphering(&self) -> bool {
        self.url.is_none() && self.signature_cipher.is_some()
    }
}

/// A format as it appears in `streamingData.formats` / `adaptiveFormats`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawFormat {
    itag: u32,
    url: Option<String>,
    signature_cipher: Option<String>,
    // The old name of `signatureCipher`.
    cipher: Option<String>,
    mime_type: String,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    bitrate: Option<u64>,
    content_length: Option<String>,
    quality_label: Option<String>,
    audio_sample_rate: Option<String>,
}

impl RawFormat {
    /// The format, as one of the formats YouTube calls `adaptive` if `adaptive`; `None` if it
    /// has neither an address nor a cipher, or an unreadable media type.
    pub(crate) fn into_format(self, adaptive: bool) -> Option<Format> {
        let (mime_type, codecs) = parse_mime_type(&self.mime_type)?;
        let kind = if !adaptive {
            StreamKind::Muxed
        } else if mime_type.starts_with("audio/") {
            StreamKind::Audio
        } else {
            StreamKind::Video
        };
        let signature_cipher = self.signature_cipher.or(self.cipher);
        if self.url.is_none() && signature_cipher.is_none() {
            return None;
        }
        Some(Format {
            itag: self.itag,
            url: self.url,
            signature_cipher,
            mime_type,
            codecs,
            kind,
            width: self.width,
            height: self.height,
            fps: self.fps,
            bitrate: self.bitrate,
            content_length: self.content_length.and_then(|n| n.parse().ok()),
            quality_label: self.quality_label,
            audio_sample_rate: self.audio_sample_rate.and_then(|n| n.parse().ok()),
        })
    }
}

/// Splits `video/mp4; codecs="avc1.42001E, mp4a.40.2"` into the type and its codecs.
fn parse_mime_type(text: &str) -> Option<(String, Vec<String>)> {
    let mut parts = text.split(';');
    let mime_type = parts.next()?.trim().to_ascii_lowercase();
    if !mime_type.contains('/') {
        return None;
    }
    let codecs = parts
        .filter_map(|part| part.trim().strip_prefix("codecs="))
        .flat_map(|list| list.trim_matches('"').split(','))
        .map(|codec| codec.trim().to_string())
        .filter(|codec| !codec.is_empty())
        .collect();
    Some((mime_type, codecs))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn raw(value: serde_json::Value) -> RawFormat {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn media_types_split_into_type_and_codecs() {
        assert_eq!(
            parse_mime_type(r#"video/mp4; codecs="avc1.42001E, mp4a.40.2""#),
            Some((
                "video/mp4".into(),
                vec!["avc1.42001E".into(), "mp4a.40.2".into()]
            ))
        );
        assert_eq!(
            parse_mime_type("audio/webm"),
            Some(("audio/webm".into(), vec![]))
        );
        assert_eq!(parse_mime_type("garbage"), None);
    }

    #[test]
    fn formats_know_their_kind_and_extension() {
        let muxed = raw(json!({
            "itag": 18, "url": "https://r.test/18", "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"",
            "width": 640, "height": 360, "fps": 30, "bitrate": 500000, "contentLength": "1234", "qualityLabel": "360p"
        }))
        .into_format(false)
        .unwrap();
        assert_eq!(muxed.kind, StreamKind::Muxed);
        assert_eq!(muxed.extension(), "mp4");
        assert_eq!(muxed.content_length, Some(1234));
        assert!(!muxed.needs_deciphering());

        let audio = raw(json!({
            "itag": 140, "signatureCipher": "s=abc&sp=sig&url=https%3A%2F%2Fr.test%2F140",
            "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "audioSampleRate": "44100"
        }))
        .into_format(true)
        .unwrap();
        assert_eq!(audio.kind, StreamKind::Audio);
        assert_eq!(audio.extension(), "m4a");
        assert_eq!(audio.audio_sample_rate, Some(44_100));
        assert!(audio.needs_deciphering());

        let video = raw(json!({"itag": 248, "url": "u", "mimeType": "video/webm; codecs=\"vp9\""}))
            .into_format(true)
            .unwrap();
        assert_eq!((video.kind, video.extension()), (StreamKind::Video, "webm"));

        assert!(
            raw(json!({"itag": 1, "mimeType": "video/mp4"}))
                .into_format(true)
                .is_none(),
            "no address at all"
        );
    }
}
