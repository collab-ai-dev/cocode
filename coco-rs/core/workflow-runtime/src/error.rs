//! Errors surfaced by the workflow runtime engine.

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;

/// A workflow-runtime failure. Tier-3 (snafu + `coco-error`); the `core/tools`
/// caller converts it into a `ToolError` at the seam.
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum WorkflowRuntimeError {
    /// Engine setup (runtime/context creation, sandbox install) failed.
    #[snafu(display("workflow engine setup failed: {message}"))]
    Setup {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The script failed to compile or evaluate.
    #[snafu(display("workflow script error: {message}"))]
    Script {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The script exceeded its wall-clock budget.
    #[snafu(display("workflow timed out after {timeout:?}"))]
    Timeout {
        timeout: std::time::Duration,
        #[snafu(implicit)]
        location: Location,
    },

    /// The run was cancelled (user stop / parent abort).
    #[snafu(display("workflow cancelled"))]
    Cancelled {
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for WorkflowRuntimeError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Setup { .. } => StatusCode::Internal,
            Self::Script { .. } => StatusCode::InvalidArguments,
            Self::Timeout { .. } => StatusCode::Timeout,
            Self::Cancelled { .. } => StatusCode::Cancelled,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
