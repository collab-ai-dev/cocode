//! Session lifecycle state.
//!
//! Consumed by the permission evaluator, memory runtime, exec server and
//! IDE bridge as well as the wire layer, so it stays in the foundation
//! crate rather than moving with the event envelope.

use serde::Deserialize;
use serde::Serialize;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Turn completed, waiting for user input.
    Idle,
    /// Agent is actively processing.
    Running,
    /// Waiting for user action (approval, question, elicitation).
    RequiresAction,
}
