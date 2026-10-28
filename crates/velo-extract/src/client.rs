//! The InnerTube clients Velo pretends to be when it asks YouTube about a video.
//!
//! YouTube answers each client differently: some get stream addresses that work as they are,
//! others get addresses whose signature and `n` parameter must first be transformed by the
//! web player's JavaScript. The extractor tries them in order and uses the first that answers.
//! YouTube retires old client versions now and then, so these values need updating over time.

use serde_json::{Value, json};

/// One InnerTube client: how it introduces itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client {
    /// A short name for messages, such as `ios`.
    pub name: &'static str,
    /// `clientName` in the request context.
    pub client_name: &'static str,
    /// `clientVersion` in the request context.
    pub client_version: &'static str,
    /// The numeric client ID, sent as `X-YouTube-Client-Name`.
    pub client_id: u32,
    /// The `User-Agent` header.
    pub user_agent: &'static str,
    /// Device fields added to the request context, as `(key, value)` pairs.
    pub device: &'static [(&'static str, &'static str)],
    /// Whether its stream addresses need the web player's JavaScript to work.
    pub needs_player: bool,
}

impl Client {
    /// The Android client for VR headsets: direct stream addresses.
    pub const ANDROID_VR: Self = Self {
        name: "android_vr",
        client_name: "ANDROID_VR",
        client_version: "1.65.10",
        client_id: 28,
        user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
        device: &[
            ("deviceMake", "Oculus"),
            ("deviceModel", "Quest 3"),
            ("androidSdkVersion", "32"),
            ("osName", "Android"),
            ("osVersion", "12L"),
        ],
        needs_player: false,
    };

    /// The iPhone app: direct stream addresses.
    pub const IOS: Self = Self {
        name: "ios",
        client_name: "IOS",
        client_version: "20.10.4",
        client_id: 5,
        user_agent: "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
        device: &[
            ("deviceMake", "Apple"),
            ("deviceModel", "iPhone16,2"),
            ("osName", "iPhone"),
            ("osVersion", "18.3.2.22D82"),
        ],
        needs_player: false,
    };

    /// The Apple Vision Pro app: direct stream addresses that YouTube serves in full, also for
    /// long videos where `IOS` and `ANDROID_VR` addresses stop after a few megabytes. It
    /// refuses videos made for kids.
    pub const VISIONOS: Self = Self {
        name: "visionos",
        client_name: "VISIONOS",
        client_version: "1.02",
        client_id: 101,
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
        device: &[
            ("deviceMake", "Apple"),
            ("deviceModel", "RealityDevice17,1"),
            ("osName", "visionOS"),
            ("osVersion", "26.5.23O471"),
        ],
        needs_player: false,
    };

    /// The TV app: needs the player's JavaScript, but plays many age-restricted videos.
    pub const TV: Self = Self {
        name: "tv",
        client_name: "TVHTML5",
        client_version: "7.20250312.16.00",
        client_id: 7,
        user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
        device: &[],
        needs_player: true,
    };

    /// The website: needs the player's JavaScript.
    pub const WEB: Self = Self {
        name: "web",
        client_name: "WEB",
        client_version: "2.20250312.04.00",
        client_id: 1,
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36",
        device: &[],
        needs_player: true,
    };

    /// The clients tried by default, in order: the ones with direct addresses first, since
    /// they need no player script.
    ///
    /// `VISIONOS` leads because its addresses download in full for every video that allows it;
    /// `IOS` addresses stop after a few megabytes for some long videos, and `ANDROID_VR` lists
    /// formats too but YouTube answers 403 when most of them are requested.
    pub const DEFAULT_ORDER: [Self; 5] = [
        Self::VISIONOS,
        Self::IOS,
        Self::ANDROID_VR,
        Self::TV,
        Self::WEB,
    ];

    /// The body of a `/youtubei/v1/player` request for `video_id`, with `signature_timestamp`
    /// from the player's JavaScript for clients that need it.
    ///
    /// `visitor_data` identifies the viewer; YouTube refuses requests without it with a bot
    /// check.
    pub fn player_request(
        &self,
        video_id: &str,
        signature_timestamp: Option<u64>,
        visitor_data: &str,
    ) -> Value {
        let mut client = json!({
            "clientName": self.client_name,
            "clientVersion": self.client_version,
            "hl": "en",
            "gl": "US",
            "visitorData": visitor_data,
        });
        for (key, value) in self.device {
            // Numbers go as numbers, as the apps send them.
            client[*key] = value
                .parse::<u64>()
                .map_or_else(|_| json!(value), |n| json!(n));
        }
        let mut body = json!({
            "context": { "client": client },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        if let Some(timestamp) = signature_timestamp {
            body["playbackContext"] = json!({
                "contentPlaybackContext": { "signatureTimestamp": timestamp }
            });
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_name_the_client_and_the_video() {
        let body = Client::ANDROID_VR.player_request("dQw4w9WgXcQ", None, "Cgt");
        assert_eq!(body["videoId"], "dQw4w9WgXcQ");
        assert_eq!(body["context"]["client"]["clientName"], "ANDROID_VR");
        assert_eq!(body["context"]["client"]["visitorData"], "Cgt");
        assert_eq!(body["context"]["client"]["androidSdkVersion"], 32);
        assert_eq!(body["context"]["client"]["osVersion"], "12L");
        assert!(body.get("playbackContext").is_none());

        let body = Client::IOS.player_request("dQw4w9WgXcQ", Some(20_160), "Cgt");
        assert_eq!(
            body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"],
            20_160
        );
    }

    #[test]
    fn clients_with_direct_addresses_come_first() {
        let order = Client::DEFAULT_ORDER;
        let first_needing = order.iter().position(|c| c.needs_player).unwrap();
        assert!(order[first_needing..].iter().all(|c| c.needs_player));
    }
}
