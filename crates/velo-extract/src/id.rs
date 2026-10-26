//! Finding the video ID in a YouTube address.

use reqwest::Url;

/// The hosts that serve YouTube watch pages and players.
const HOSTS: [&str; 5] = [
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "www.youtube-nocookie.com",
];

/// The 11-character video ID in a YouTube address, or `None` if `url` isn't a YouTube video
/// address.
///
/// Understands `watch?v=`, `/shorts/`, `/embed/`, `/live/`, `/v/` and `youtu.be/` addresses
/// on the usual YouTube hosts.
pub fn video_id(url: &str) -> Option<String> {
    let url = Url::parse(url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let mut segments = url.path_segments()?.filter(|s| !s.is_empty());
    let id = if host == "youtu.be" {
        segments.next()?.to_string()
    } else if HOSTS.contains(&host.as_str()) {
        match segments.next()? {
            "watch" => url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())?,
            "shorts" | "embed" | "live" | "v" => segments.next()?.to_string(),
            _ => return None,
        }
    } else {
        return None;
    };
    is_video_id(&id).then_some(id)
}

/// Whether `id` has the shape of a video ID: 11 characters of `A–Z a–z 0–9 - _`.
pub fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids_come_from_every_kind_of_address() {
        let id = Some("dQw4w9WgXcQ".to_string());
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=42",
            "http://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ?start=10",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ?si=abc",
            "https://www.youtube.com/v/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=1",
            "  https://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ  ",
        ] {
            assert_eq!(video_id(url), id, "{url}");
        }
    }

    #[test]
    fn other_addresses_have_no_video_id() {
        for url in [
            "https://www.youtube.com/",
            "https://www.youtube.com/@channel",
            "https://www.youtube.com/playlist?list=PL123",
            "https://www.youtube.com/watch?v=short",
            "https://www.youtube.com/watch?v=dQw4w9WgXc!",
            "https://youtu.be/",
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
            "ftp://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "not a url",
        ] {
            assert_eq!(video_id(url), None, "{url}");
        }
    }
}
