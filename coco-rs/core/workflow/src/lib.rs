//! Dynamic workflow source loading and validation.

mod meta;
mod source;

pub use meta::WorkflowMeta;
pub use meta::WorkflowPhaseMeta;
pub use meta::WorkflowScript;
pub use meta::parse_workflow_meta;
pub use meta::parse_workflow_script;
pub use source::MAX_WORKFLOW_SOURCE_BYTES;
pub use source::WorkflowRegistryEntry;
pub use source::WorkflowSourceInput;
pub use source::WorkflowSourceKind;
pub use source::WorkflowSourceSpec;
pub use source::list_workflows;
pub use source::resolve_workflow_source;
pub use source::workflow_dirs_hint;

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;

#[stack_trace_debug]
#[derive(Snafu)]
pub enum WorkflowError {
    #[snafu(display("workflow source is required"))]
    MissingSource {
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow '{name}' was not found{available}"))]
    NamedWorkflowNotFound {
        name: String,
        available: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow script path cannot be a UNC path: {path}"))]
    UncPath {
        path: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow source exceeds {limit} bytes: {actual} bytes"))]
    SourceTooLarge {
        limit: usize,
        actual: usize,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow source contains invalid UTF-8"))]
    InvalidUtf8 {
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("failed to read workflow source {path}: {message}"))]
    ReadSource {
        path: String,
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow source has TypeScript syntax errors"))]
    Syntax {
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow meta must be the first statement: export const meta = {{...}}"))]
    MissingMeta {
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow meta must be a pure object literal: {message}"))]
    InvalidMeta {
        message: String,
        #[snafu(implicit)]
        location: Location,
    },
    #[snafu(display("workflow source uses nondeterministic API: {api}"))]
    NondeterministicApi {
        api: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for WorkflowError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::ReadSource { .. } => StatusCode::FileNotFound,
            Self::MissingSource { .. }
            | Self::NamedWorkflowNotFound { .. }
            | Self::UncPath { .. }
            | Self::SourceTooLarge { .. }
            | Self::InvalidUtf8 { .. }
            | Self::Syntax { .. }
            | Self::MissingMeta { .. }
            | Self::InvalidMeta { .. }
            | Self::NondeterministicApi { .. } => StatusCode::InvalidArguments,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T> = std::result::Result<T, WorkflowError>;

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
