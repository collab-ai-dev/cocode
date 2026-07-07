//! wire-parsing tool-input parsing helper for the streaming / non-streaming
//! engine paths.
//!
//! Wraps [`coco_utils_json_repair::parse_with_repair`] with the same raw
//! preservation fallback used by provider-utils. Empty input maps to `{}` for
//! parameterless tools; non-empty unrecoverable input stays as `Value::String`
//! so schema validation/tool-runtime coercion can classify it with full raw
//! context.
//!
//! Parallel to `vercel_ai_provider_utils::parse_tool_arguments_or_empty`
//! (used inside provider adapters). Both delegate to `llm_json` so
//! behavioral drift is bounded; the duplication exists because the
//! layering rule forbids provider-utils from depending on
//! `coco-utils-*`.

use coco_utils_json_repair::RepairOutcome;
use coco_utils_json_repair::parse_with_repair;
use serde_json::Value;

/// Parse tool-call arguments with two fallback rules — see the doc
/// on the parallel [`vercel_ai_provider_utils::parse_tool_arguments_or_empty`]
/// for the rationale (this is the engine-side mirror used by the
/// streaming reconstruction seam).
///
/// 1. Empty / whitespace-only input → `Value::Object({})`
/// 2. Non-empty unrecoverable → `Value::String(raw)` (preserves the
///    model's raw output for downstream diagnostics and the
///    `<tool_use_error>` body)
pub fn parse_tool_arguments_or_empty(raw: &str, tool_name: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    match parse_with_repair(raw) {
        Ok((v, RepairOutcome::Clean)) => v,
        Ok((v, RepairOutcome::Repaired)) => {
            tracing::warn!(
                target: "coco_query::tool_input",
                tool_name,
                args_bytes = raw.len(),
                "tool-call arguments JSON required repair before parse"
            );
            v
        }
        Err(err) => {
            tracing::warn!(
                target: "coco_query::tool_input",
                tool_name,
                args_bytes = raw.len(),
                error = %err,
                "tool-call arguments parse failed; preserving raw string for downstream diagnostics"
            );
            Value::String(raw.to_string())
        }
    }
}
