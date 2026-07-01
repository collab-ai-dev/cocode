//! Tool input normalization + schema validation.
//!
//! Sits between the provider-adapter-driven wire parsing (which parses raw
//! `arguments` strings into [`coco_llm_types::ToolCallPart::input`]
//! values, falling back to `{}` on parse failure) and the per-tool
//! execution gate in [`tool_call_preparer`].
//!
//! - [`normalize_value_string`] — recursive `Value::String` parse for
//!   providers that occasionally nest stringified JSON inside the tool-call
//!   `input` field.
//! - [`validate_tool_call`] — schema check + NoSuchTool fallback, without
//!   the schema-not-sent hint (deferred-tool registry not yet ported).
//! - [`format_schema_error`] — human-readable schema error formatting.
//!
//! Failure paths populate [`coco_llm_types::ToolInputInvalidReason`] so error
//! wrap ([`tool_call_preparer::prepare_one_pending_tool_call`]) can pick the
//! right `<tool_use_error>` wrap prefix without string-matching.

use std::sync::Arc;

use coco_llm_types::ToolCallPart;
use coco_llm_types::ToolInputInvalidReason;
use coco_tool_runtime::SchemaIssue;
use coco_tool_runtime::ValidatedInput;
use coco_tool_runtime::traits::DynTool;
// Canonical formatter lives next to `SchemaIssue` in `coco-tool-runtime`;
// re-exported here for the preparer + tests.
pub use coco_tool_runtime::format_schema_error;
use serde_json::Value;

use crate::tool_input_parse::parse_tool_arguments_or_empty;

pub(crate) fn format_schema_error_with_steer(
    tool_name: &str,
    tool: &dyn DynTool,
    input: &Value,
    issues: &[SchemaIssue],
) -> String {
    let mut message = format_schema_error(tool_name, issues);
    if let Some(steer) = tool.validation_error_steer(input) {
        message.push_str("\n\n");
        message.push_str(&steer);
    }
    message
}

/// Recover stringified JSON nested inside what the schema expects to
/// be an object/array.
///
/// - `Value::String(s)` → try [`parse_tool_arguments_or_empty`];
///   when it produces an `Object`/`Array`, overwrite `input` with the
///   parsed value. Otherwise keep the original `String` and let the
///   schema validator surface a type-mismatch error.
/// - Other variants pass through unchanged.
pub fn normalize_value_string(input: &mut Value) {
    if let Value::String(s) = input {
        let s_owned = std::mem::take(s);
        let recovered = parse_tool_arguments_or_empty(&s_owned, "(value-string)");
        match recovered {
            Value::Object(_) | Value::Array(_) => *input = recovered,
            // Couldn't recover into a structured value. Restore the
            // original string so the schema validator can flag the
            // type mismatch.
            _ => *input = Value::String(s_owned),
        }
    }
}

/// schema validation entry point. Returns the coerced, schema-validated
/// input on success; on failure sets `invalid` / `invalid_reason` on `tc`
/// and returns `None`.
///
/// `tc.input` itself is never mutated: the persisted assistant message
/// keeps the wire shape (a freeform tool call's raw string round-trips
/// verbatim to the provider), while everything downstream — permission
/// evaluation, hooks' `updated_input` re-validation, execution — consumes
/// the returned [`ValidatedInput`].
///
/// Returns `None` without classifying when the call is already invalid
/// from wire parsing — the earlier provider-side classification stands.
pub fn validate_tool_call(
    tc: &mut ToolCallPart,
    tool: Option<&Arc<dyn DynTool>>,
) -> Option<ValidatedInput> {
    if tc.invalid {
        return None;
    }

    // 1. NoSuchTool — short-circuit before touching the schema.
    let Some(tool) = tool else {
        tc.invalid = true;
        tc.invalid_reason = Some(ToolInputInvalidReason::NoSuchTool {
            tool_name: tc.tool_name.clone(),
        });
        return None;
    };

    // 2. Freeform/custom-tool coercion vs. JSON string-recovery — mutually
    //    exclusive, coercion first.
    //
    //    A freeform tool (apply_patch) is called with a bare string (the raw
    //    `*** Begin Patch …` envelope); `ValidatedInput::validate` wraps it
    //    into the typed JSON its schema expects (`{patch: raw}`) so schema
    //    validation + `Self::Input` deserialization succeed.
    //
    //    codex-rs routes such custom tool calls to a dedicated raw-string
    //    `ToolPayload::Custom { input }` that is NEVER parsed as JSON — only
    //    `Function` arguments are. We mirror that: when the tool coerces a
    //    raw string (i.e. it's freeform), DO NOT run `normalize_value_string`,
    //    which would try to JSON-parse the patch envelope and could mangle a
    //    body that happens to look like JSON. Only non-coercing (function)
    //    tools get string-recovery, where nested stringified-JSON is real.
    let candidate = match &tc.input {
        Value::String(raw) if tool.coerce_raw_string_input(raw).is_none() => {
            let mut recovered = tc.input.clone();
            normalize_value_string(&mut recovered);
            recovered
        }
        other => other.clone(),
    };

    // 3. Coercion + schema validation, fused in the [`ValidatedInput`]
    //    constructor — synchronous and lock-free; the validator is owned by
    //    the schema (v4.2). A schema-compile failure is impossible here: a
    //    tool is only registered if its schema compiled at construction, so
    //    the only outcomes are clean or classified issues.
    let validation_input = candidate.clone();
    match ValidatedInput::validate(tool.as_ref(), candidate) {
        Ok(validated) => Some(validated),
        Err(issues) => {
            let message = format_schema_error_with_steer(
                &tc.tool_name,
                tool.as_ref(),
                &validation_input,
                &issues,
            );
            tc.invalid = true;
            tc.invalid_reason = Some(ToolInputInvalidReason::SchemaViolation { message });
            None
        }
    }
}

#[cfg(test)]
#[path = "tool_input_validate.test.rs"]
mod tests;
