//! Queues: ordered lists of downloads that run a limited number at a time, on a schedule.

use serde::{Deserialize, Serialize};

use crate::DownloadId;

/// Identifies a queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QueueId(pub u32);

impl QueueId {
    /// The built-in main download queue, which always exists.
    pub const MAIN: Self = Self(0);
}

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
    /// When the queue starts and stops by itself, and what happens when it is done.
    pub schedule: Schedule,
}

impl Queue {
    /// The built-in "Main download queue", which runs one download at a time.
    pub fn main() -> Self {
        Self::new(QueueId::MAIN, "Main download queue")
    }

    /// An empty queue that runs one download at a time and has no schedule.
    pub fn new(id: QueueId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            max_concurrent: 1,
            downloads: Vec::new(),
            schedule: Schedule::default(),
        }
    }
}

/// A time of day, in local time.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TimeOfDay {
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
}

impl TimeOfDay {
    /// The time `hour:minute`, or `None` if either is out of range.
    pub fn new(hour: u8, minute: u8) -> Option<Self> {
        (hour < 24 && minute < 60).then_some(Self { hour, minute })
    }

    /// Parses `HH:MM`.
    pub fn parse(text: &str) -> Option<Self> {
        let (hour, minute) = text.trim().split_once(':')?;
        Self::new(hour.parse().ok()?, minute.parse().ok()?)
    }
}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

/// When a queue starts and stops by itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// Start the queue at this time; never by itself if `None`.
    pub start_at: Option<TimeOfDay>,
    /// The weekdays to start on, Monday first. With no day set, the queue starts once, at the
    /// next `start_at`, and then the start time is cleared.
    pub days: [bool; 7],
    /// Stop the queue at this time, pausing what it runs.
    pub stop_at: Option<TimeOfDay>,
    /// Quit Velo when the queue has finished.
    pub exit_when_done: bool,
    /// Turn the computer off when the queue has finished.
    pub shutdown_when_done: bool,
}

impl Schedule {
    /// Whether the queue starts once rather than on chosen weekdays.
    pub fn is_once(&self) -> bool {
        !self.days.iter().any(|&d| d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_of_day_parse_and_print() {
        assert_eq!(TimeOfDay::parse(" 7:05 "), TimeOfDay::new(7, 5));
        assert_eq!(TimeOfDay::new(23, 59).unwrap().to_string(), "23:59");
        assert_eq!(TimeOfDay::parse("24:00"), None);
        assert_eq!(TimeOfDay::parse("12:60"), None);
        assert_eq!(TimeOfDay::parse("noon"), None);
    }
}
