//! Crate-local error type for skill loading + extraction. Wraps
//! `std::io::Error` via `#[from]` so internal `?` chains work without
//! explicit `map_err`, and exposes `ErrorExt` for unified `StatusCode`.

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkillsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),

    #[error("{message}")]
    Generic { message: String },
}

impl SkillsError {
    pub fn generic(message: impl Into<String>) -> Self {
        Self::Generic {
            message: message.into(),
        }
    }
}

impl StackError for SkillsError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for SkillsError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io(_) => StatusCode::IoError,
            Self::Join(_) => StatusCode::Internal,
            Self::Generic { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
