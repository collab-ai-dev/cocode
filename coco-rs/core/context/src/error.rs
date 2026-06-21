//! Typed error for the coco-context crate.
//!
//! Tier 3 main-trunk: implements `coco_error::ErrorExt` so callers can
//! classify failures by `StatusCode`.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("{message}")]
    Generic { message: String },

    #[error("io error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("json error: {source}")]
    Json {
        #[from]
        source: serde_json::Error,
    },

    #[error("git failed: {message}")]
    GitFailed { message: String },

    #[error("path error at {path}: {message}")]
    PathError { path: PathBuf, message: String },
}

impl ContextError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }

    pub fn git_failed(message: impl Into<String>) -> Self {
        Self::GitFailed {
            message: message.into(),
        }
    }
}

impl StackError for ContextError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for ContextError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::GitFailed { .. } => StatusCode::External,
            Self::PathError { .. } => StatusCode::InvalidArguments,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = ContextError> = std::result::Result<T, E>;
