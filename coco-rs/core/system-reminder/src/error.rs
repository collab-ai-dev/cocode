//! Error types for the system reminder crate.
//!
//! All variants map to status codes in the `13_xxx` (SystemReminder) range —
//! see `coco-error/src/status_code.rs`.

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use snafu::Snafu;

/// Errors that can occur inside the system-reminder subsystem.
#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub(crate)), module)]
pub enum SystemReminderError {
    /// A generator exceeded its configured per-turn timeout.
    ///
    /// Each parallel attachment gather has a 1000ms timeout. Timed-out
    /// generators contribute zero reminders; the turn continues normally.
    #[snafu(display("Reminder generator {generator} timed out after {timeout_ms}ms"))]
    GeneratorTimeout {
        generator: String,
        timeout_ms: i64,
        #[snafu(implicit)]
        location: Location,
    },

    /// A generator's `generate()` returned an error.
    #[snafu(display("Reminder generator {generator} failed: {message}"))]
    GeneratorFailed {
        generator: String,
        message: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The throttle manager's lock was poisoned by a panicking task.
    ///
    /// Recovery: the orchestrator falls back to "allow generation" so the
    /// turn is never blocked by a poisoned state.
    #[snafu(display("Throttle state poisoned for {attachment_type}"))]
    ThrottlePoisoned {
        attachment_type: String,
        #[snafu(implicit)]
        location: Location,
    },

    /// The `GeneratorContext` was missing a field that this generator requires.
    #[snafu(display("Generator {generator} requires context field: {field}"))]
    InvalidContext {
        generator: String,
        field: String,
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for SystemReminderError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::GeneratorTimeout { .. } => StatusCode::ReminderGeneratorTimeout,
            Self::GeneratorFailed { .. } => StatusCode::ReminderGeneratorFailed,
            Self::ThrottlePoisoned { .. } => StatusCode::ReminderThrottlePoisoned,
            Self::InvalidContext { .. } => StatusCode::ReminderInvalidContext,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T> = std::result::Result<T, SystemReminderError>;

#[cfg(test)]
#[path = "error.test.rs"]
mod tests;
