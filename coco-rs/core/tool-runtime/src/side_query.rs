//! Side-query trait — async LLM inference abstraction for tools.
//!
//! **Split design**:
//! - Data types (`SideQueryRequest`, `SideQueryResponse`, etc.) → `coco-types`
//!   (shared by both `coco-tool` and `coco-permissions`)
//! - Async trait (`SideQuery`) → here in `coco-tool` (needs `async-trait`)
//!
//! **Dependency flow**:
//! ```text
//! coco-types         (data types: request, response, message, etc.)
//!     ↑
//! coco-tool          (defines async SideQuery trait, puts Arc<dyn> on ToolUseContext)
//! coco-permissions   (uses data types via coco-types; uses callbacks that accept same types)
//!     ↑
//! coco-inference     (implements SideQuery trait)
//!     ↑
//! coco-executor      (wires implementation into ToolUseContext)
//! ```

use std::sync::Arc;

// Re-export data types for convenience — callers can import from coco_tool_runtime
// without adding coco-types as a direct dependency.
pub use coco_types::Capability;
pub use coco_types::ModelRole;
pub use coco_types::SideQueryOutputFormat;
pub use coco_types::SideQueryRequest;
pub use coco_types::SideQueryResponse;
pub use coco_types::SideQueryStopReason;
pub use coco_types::SideQueryToolDef;
pub use coco_types::SideQueryUsage;

/// Trait for making LLM side-queries from tools.
///
/// Implementations live in the inference/executor layer. Tools access
/// this via `ToolUseContext.side_query`.
///
/// **When to use `SideQuery` vs callbacks:**
/// - Use `SideQuery` in tools that make repeated LLM calls (Bash safety,
///   Edit retry, Agent orchestration)
/// - Use callbacks (`FnOnce`) for one-shot callers (classifier, explainer)
///   that want custom request/response types
/// - Both use the same data types from `coco-types::side_query`
#[async_trait::async_trait]
pub trait SideQuery: Send + Sync {
    /// Execute a side-query against the LLM.
    async fn query(
        &self,
        request: SideQueryRequest,
    ) -> Result<SideQueryResponse, coco_error::BoxedError>;

    /// Get the default model ID for side queries.
    fn model_id(&self) -> &str;

    /// Whether the resolved model for `role` (or the default model
    /// when `role` is `None`) declares the given [`Capability`].
    ///
    /// Used by callers that need to pick between provider-supported
    /// paths and the multi-LLM fallback at request build time — most
    /// notably the memory recall ranker choosing between native
    /// `output_format` (when [`Capability::StructuredOutput`] is set)
    /// and `with_forced_tool` (universally supported).
    ///
    /// Default impl returns `false` so test doubles
    /// ([`NoOpSideQuery`], unit-test stubs) keep compiling without
    /// caring about capability semantics. Real implementations
    /// override against the live model registry.
    fn supports_capability(&self, role: Option<ModelRole>, capability: Capability) -> bool {
        let _ = (role, capability);
        false
    }
}

/// Shared handle type for `ToolUseContext`.
pub type SideQueryHandle = Arc<dyn SideQuery>;

/// A no-op implementation that returns an error. Used in test contexts.
#[derive(Debug, Clone)]
pub struct NoOpSideQuery;

#[async_trait::async_trait]
impl SideQuery for NoOpSideQuery {
    async fn query(
        &self,
        _request: SideQueryRequest,
    ) -> Result<SideQueryResponse, coco_error::BoxedError> {
        Err(Box::new(coco_error::PlainError::new(
            "LLM side-query not available in this context",
            coco_error::StatusCode::Internal,
        )))
    }

    fn model_id(&self) -> &str {
        "none"
    }
}

// ── Bridge: SideQuery → callback ──

/// Create a one-shot callback from a `SideQuery` handle.
///
/// This bridges the `SideQuery` trait with the callback pattern used
/// by the permission classifier and explainer. The executor creates
/// a `SideQueryHandle` once and uses this function to generate
/// callbacks for each classifier/explainer invocation.
///
/// Returns a closure `(system, user_prompt) → Result<String>` that
/// sends a simple text query and returns the text response.
pub fn side_query_to_text_callback(
    handle: SideQueryHandle,
    query_source: String,
) -> impl FnOnce(
    String,
    String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> {
    move |system: String, user_prompt: String| {
        Box::pin(async move {
            let request = SideQueryRequest::simple(&system, &user_prompt, &query_source);
            let response = handle.query(request).await.map_err(|e| e.to_string())?;
            // Prefer text response; fall back to first tool use input if text is empty
            match &response.text {
                Some(text) if !text.is_empty() => Ok(text.clone()),
                _ => {
                    if let Some(tool_json) = response.first_tool_input() {
                        Ok(tool_json.to_string())
                    } else {
                        Err("no text or tool response from LLM".into())
                    }
                }
            }
        })
    }
}
