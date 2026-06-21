//! Typed error for the coordinator crate.
//!
//! Tier 3 main-trunk: implements `coco_error::ErrorExt` so failures
//! classify by `StatusCode`. Boundary-converts via `coco_error::boxed`
//! when implementing trait methods that return `BoxedError`.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoordinatorError {
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

    #[error("subprocess failed: {message}")]
    SubprocessFailed { message: String },

    #[error("lock acquisition failed: {message}")]
    LockFailed { message: String },

    #[error("teammate not found: {name}")]
    TeammateNotFound { name: String },
}

impl CoordinatorError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }

    pub fn subprocess_failed(message: impl Into<String>) -> Self {
        Self::SubprocessFailed {
            message: message.into(),
        }
    }
}

impl StackError for CoordinatorError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for CoordinatorError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::SubprocessFailed { .. } => StatusCode::External,
            Self::LockFailed { .. } => StatusCode::IoError,
            Self::TeammateNotFound { .. } => StatusCode::FileNotFound,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = CoordinatorError> = std::result::Result<T, E>;
