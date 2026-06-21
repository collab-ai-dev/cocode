//! Crate-local error type for session / transcript / history / recovery /
//! title persistence. Wraps `std::io::Error` and `serde_json::Error` via
//! `#[from]` so internal `?` chains propagate without explicit `map_err`,
//! and exposes the result through `coco_error::ErrorExt` for the unified
//! `StatusCode` classification.

use std::path::PathBuf;

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Error raised by a non-fs backend (DB / HTTP / object store). Carries
    /// the backend's own `StatusCode` so classification survives the
    /// boundary instead of collapsing to `Generic`. `Box<dyn ErrorExt>`
    /// does not auto-impl `std::error::Error`, so this is a plain
    /// `Display`-formatted variant (no `transparent` / `#[from]`).
    #[error("backend error: {0}")]
    Backend(coco_error::BoxedError),

    #[error("transcript not found: {path}")]
    TranscriptNotFound { path: PathBuf },

    #[error("transcript path missing UUID component: {path}")]
    InvalidTranscriptPath { path: PathBuf },

    #[error("session duration exceeds current time")]
    DurationOverflow,

    #[error("{message}")]
    Generic { message: String },
}

impl SessionError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }
}

impl StackError for SessionError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for SessionError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io(_) => StatusCode::IoError,
            Self::Json(_) => StatusCode::InvalidJson,
            Self::Backend(e) => e.status_code(),
            Self::TranscriptNotFound { .. } => StatusCode::FileNotFound,
            Self::InvalidTranscriptPath { .. } => StatusCode::InvalidArguments,
            Self::DurationOverflow => StatusCode::Internal,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
