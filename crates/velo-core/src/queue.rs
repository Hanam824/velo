//! Queues: ordered lists of downloads that run a limited number at a time.

use serde::{Deserialize, Serialize};

use crate::DownloadId;

/// Identifies a queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QueueId(pub u32);

/// An ordered list of downloads that runs at most `max_concurrent` of them at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    /// The queue's id.
    pub id: QueueId,
    /// Name shown in the categories tree and the scheduler.
    pub name: String,
    /// How many downloads in the queue run at the same time; at least 1.
    pub max_concurrent: u32,
    /// The downloads, in the order they start.
    pub downloads: Vec<DownloadId>,
}

impl Queue {
    /// The built-in "Main download queue", which runs one download at a time.
    pub fn main() -> Self {
        Self {
            id: QueueId(0),
            name: "Main download queue".into(),
            max_concurrent: 1,
            downloads: Vec::new(),
        }
    }
}
