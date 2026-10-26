//! Domain types shared by every Velo crate: downloads, segments, categories, queues and settings.
//!
//! This crate does no I/O. Types derive `serde` traits so that `velo-store` can persist them and
//! `velo-ipc` can send them.

pub mod batch;
mod category;
mod download;
mod event;
pub mod format;
pub mod links;
mod queue;
mod segment;
mod settings;
mod source;

pub use category::Category;
pub use download::{Download, DownloadId, DownloadState};
pub use event::{EngineEvent, ProgressUpdate};
pub use queue::{Queue, QueueId, Schedule, TimeOfDay};
pub use segment::{Segment, SegmentId, SegmentState};
pub use settings::{Appearance, ConnectionLimit, ProxyMode, ProxySettings, Settings};
pub use source::{Resolver, Resolving, Source};

/// Errors from Velo's domain layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A value given to the domain layer was out of range or malformed.
    #[error("invalid value: {0}")]
    Invalid(String),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
