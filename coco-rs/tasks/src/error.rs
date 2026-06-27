//! Typed error for the coco-tasks crate.
//!
//! Tier 3 main-trunk: implements `coco_error::ErrorExt` so callers can
//! classify failures by `StatusCode`.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TasksError {
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

    #[error("task file not found at {path}")]
    TaskNotFound { path: PathBuf },

    #[error("lock acquisition failed for {path}: {message}")]
    LockFailed { path: PathBuf, message: String },
}

impl TasksError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }
}

impl StackError for TasksError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for TasksError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::TaskNotFound { .. } => StatusCode::FileNotFound,
            Self::LockFailed { .. } => StatusCode::IoError,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T, E = TasksError> = std::result::Result<T, E>;
