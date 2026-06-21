//! Common error utilities for coco-rs.

pub mod ext;
pub mod status_code;

// Re-export snafu and stack-trace macro for convenience
pub use coco_stack_trace_macro::stack_trace_debug;
pub use snafu;
pub use snafu::Location;

pub use ext::BoxedError;
pub use ext::BoxedErrorSource;
pub use ext::ErrorExt;
pub use ext::PlainError;
pub use ext::StackError;
pub use ext::boxed;
pub use ext::boxed_err;
pub use status_code::StatusCode;
