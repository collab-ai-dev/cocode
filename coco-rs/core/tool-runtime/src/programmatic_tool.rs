use std::sync::Arc;

use serde_json::Value;

/// Typed failure returned to a sandboxed programmatic caller.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProgrammaticToolCallError {
    #[error("programmatic tool '{tool}' is unavailable: {reason}")]
    Unavailable { tool: String, reason: String },
    #[error("programmatic tool '{tool}' is not provably read-only for this input")]
    NotReadOnly { tool: String },
    #[error("programmatic tool '{tool}' input is invalid: {reason}")]
    InvalidInput { tool: String, reason: String },
    #[error("programmatic tool '{tool}' failed: {reason}")]
    Failed { tool: String, reason: String },
}

/// Narrow callback from a sandboxed language runtime into the application's
/// canonical tool pipeline. Implementations must re-run validation,
/// permissions, hooks, cancellation, and result shaping; callers never receive
/// a raw [`DynTool`](crate::DynTool).
#[async_trait::async_trait]
pub trait ProgrammaticToolCallHandle: Send + Sync + 'static {
    async fn call_read_only(
        &self,
        tool_name: String,
        input: Value,
    ) -> Result<Value, ProgrammaticToolCallError>;
}

pub type ProgrammaticToolCallHandleRef = Arc<dyn ProgrammaticToolCallHandle>;
