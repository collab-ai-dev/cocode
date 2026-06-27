//! Tier-3 error type for the session-trace service (snafu + `coco-error`).

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;

pub use session_trace_error::*;

/// Errors raised while writing or replaying a session-trace bundle.
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub), module)]
pub enum SessionTraceError {
    /// Filesystem I/O failure (create dir, open/append/read bundle files).
    #[snafu(display("session-trace I/O error: {message}"))]
    Io {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// (De)serialization of the manifest or a trace record failed.
    #[snafu(display("session-trace serialization error: {message}"))]
    Serde {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The bundle is structurally invalid (missing manifest, bad schema, …).
    #[snafu(display("malformed trace bundle: {message}"))]
    Malformed {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for SessionTraceError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Serde { .. } => StatusCode::Internal,
            Self::Malformed { .. } => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T> = std::result::Result<T, SessionTraceError>;
