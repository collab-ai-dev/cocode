//! Errors for output style loading.
//!
//! Tier 3 (main trunk) per `CLAUDE.md` error policy: implements
//! [`coco_error::ErrorExt`] + [`coco_error::StackError`] so callers can
//! classify failures with a [`StatusCode`]. Internally uses `thiserror`
//! for terse derives — `snafu` adds no value for the small surface
//! here (two variants, no nested context).

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputStylesError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid frontmatter in {path}: {message}")]
    InvalidFrontmatter { path: PathBuf, message: String },
}

impl StackError for OutputStylesError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for OutputStylesError {
    fn status_code(&self) -> StatusCode {
        match self {
            // I/O failures while reading style markdown — non-fatal at
            // resolve time (caller logs + continues), but classify as
            // resource so retry/backoff sees them consistently.
            Self::Io { .. } => StatusCode::IoError,
            // Frontmatter / parse-level failures are user input
            // problems, not internal bugs.
            Self::InvalidFrontmatter { .. } => StatusCode::ParseError,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
