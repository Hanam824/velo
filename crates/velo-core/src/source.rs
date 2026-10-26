//! Where a download's address comes from when it isn't a plain, lasting URL.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// What a download was made from, when its address must be worked out again before each start
/// because the server's addresses expire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Streams of a YouTube video.
    #[serde(rename = "youtube")]
    YouTube {
        /// The 11-character video ID.
        video_id: String,
        /// The formats (YouTube's itags) to download: one stream, or video then audio to join.
        itags: Vec<u32>,
    },
}

impl Source {
    /// The most bytes one request should ask for; YouTube slows down longer ranges.
    pub fn max_request_size(&self) -> Option<u64> {
        match self {
            Self::YouTube { .. } => Some(10 * 1024 * 1024),
        }
    }
}

/// A future returned by [`Resolver::resolve`].
pub type Resolving<'a> = Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + 'a>>;

/// Works out the current addresses of a [`Source`]; the engine asks before each start.
pub trait Resolver: Send + Sync + std::fmt::Debug {
    /// The addresses for `source`, one per stream in its order, or why there are none.
    fn resolve<'a>(&'a self, source: &'a Source) -> Resolving<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_tagged_json() {
        let source = Source::YouTube {
            video_id: "dQw4w9WgXcQ".into(),
            itags: vec![137, 140],
        };
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"youtube","video_id":"dQw4w9WgXcQ","itags":[137,140]}"#
        );
        assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);
        assert_eq!(source.max_request_size(), Some(10 << 20));
    }
}
