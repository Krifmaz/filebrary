//! Error type for preview generation.

use std::io;

/// Everything that can go wrong while producing a preview.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    /// The file type has no preview support at all.
    #[error("no preview available for this file type")]
    Unsupported,
    /// The file type is supported, but the external tool it needs is not
    /// installed (for example `ffmpeg` in the slim image).
    #[error("preview needs `{0}`, which is not installed")]
    ToolMissing(&'static str),
    /// The input could not be decoded (corrupt, truncated, or a decoder limit
    /// such as maximum dimensions was hit).
    #[error("could not decode file: {0}")]
    Decode(String),
    /// An external tool exited with an error.
    #[error("`{tool}` failed: {message}")]
    Tool {
        /// Name of the tool that failed.
        tool: &'static str,
        /// Last lines of its stderr, trimmed.
        message: String,
    },
    /// An external tool ran longer than its time budget and was killed.
    #[error("`{0}` timed out")]
    Timeout(&'static str),
    /// The file is larger than the configured limit for this kind of preview.
    #[error("file is too large to preview")]
    TooLarge,
    /// A previous attempt failed recently and the failure is cached.
    #[error("preview failed recently, not retrying yet")]
    RecentlyFailed,
    /// Filesystem or pipe error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl PreviewError {
    /// Whether this error is worth remembering in the negative cache, so the
    /// same broken file is not re-processed on every request.
    pub fn is_cacheable(&self) -> bool {
        matches!(
            self,
            PreviewError::Decode(_) | PreviewError::Tool { .. } | PreviewError::Timeout(_)
        )
    }
}

impl From<image::ImageError> for PreviewError {
    fn from(e: image::ImageError) -> Self {
        match e {
            image::ImageError::IoError(io) => PreviewError::Io(io),
            other => PreviewError::Decode(other.to_string()),
        }
    }
}

/// Shorthand result type.
pub type Result<T, E = PreviewError> = std::result::Result<T, E>;
