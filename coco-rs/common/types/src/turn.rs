//! Why an in-flight turn stopped.
//!
//! Consumed by the tool layer (abort signals) as well as the event layer
//! (`TurnEnded`), so it is owned here rather than by either one.

use serde::Deserialize;
use serde::Serialize;

/// Why a turn was aborted. Lets consumers distinguish user cancel,
/// submit interrupt, permission abort, and system pre-emption.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnAbortReason {
    /// User-initiated cancel (Ctrl+C in the TUI, `control/interrupt`
    /// in the SDK). The only reason that may trigger auto-restore.
    UserCancel,
    /// Streaming submit interruption: the user submitted new input while
    /// all running tools were cancel-interruptible.
    SubmitInterrupt,
    /// System pre-empted the in-flight turn so another session-level
    /// operation can run (Clear / Compact / Rewind / Shutdown / new
    /// SubmitInput). Auto-restore is suppressed — the user did not
    /// request a rewind.
    SystemPreempt,
    /// Permission flow aborted the turn instead of returning a normal
    /// model-visible denial.
    PermissionAbort,
    /// Turn moved to the background.
    Background,
}
