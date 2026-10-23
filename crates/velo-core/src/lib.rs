//! Domain types shared by every Velo crate: downloads, segments, categories, queues and settings.

/// Errors from Velo's domain layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A value given to the domain layer was out of range or malformed.
    #[error("invalid value: {0}")]
    Invalid(String),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
