//! LLM-output JSON repair for provider adapters.
//!
//! Wraps the [`llm_json`] crate (Rust port of the Python `json_repair`
//! library). Used by provider adapters when converting the model's
//! raw `arguments` / `input_json` string into a `serde_json::Value`
//! for [`vercel_ai_provider::ToolCallPart`].
//!
//! **Coco-rs-specific deviation from upstream `@ai-sdk/provider-utils`:**
//! exposes aggressive repair (markdown fence stripping, single-quote →
//! double-quote conversion, trailing-comma fix, Python literal mapping,
//! truncation completion) because coco-rs targets diverse OpenAI-
//! compatible endpoints (GLM, Doubao, DeepSeek, Groq, xAI, Ollama)
//! whose tool-call argument strings are messier than first-party
//! OpenAI / Anthropic output.
//!
//! **Streaming policy**: never call this on a still-streaming buffer.
//! The repairer interprets `{"a":1,` as "needs closing brace" and
//! produces `{"a":1}`, dropping any field the model was still emitting.
//! Always defer to the terminal event (`ToolInputEnd` / `ToolCall` /
//! `content_block_stop`).
//!
//! Adapters that fail this call fall back to `Value::Object({})` (not
//! `invalid = true`) so the schema-validation schema validator reports the
//! missing fields specifically.
//!
//! Parallel implementation: `coco-utils-json-repair` lives one layer
//! higher (`utils/`) and is used by `app/query` for schema-validation work; we
//! cannot share it from here because `vercel-ai-provider-utils` must
//! stay free of `coco-*` dependencies per the workspace layering rule.
//! Both wrappers delegate to `llm_json::repair_json` so behavioural
//! drift is bounded.

use llm_json::RepairOptions;
use llm_json::repair_json;
use serde_json::Value;

/// Tag whether the parse needed repair, for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Strict [`serde_json::from_str`] succeeded.
    Clean,
    /// Required one or more [`llm_json`] repair passes.
    Repaired,
}

/// Parse JSON, attempting [`llm_json`] repair on strict-parse failure.
///
/// `Err` is returned when the input is empty / whitespace-only or when
/// even repaired text still fails to parse — adapters call sites treat
/// `Err` as "fall back to `Value::Object({})`".
pub fn parse_with_repair(raw: &str) -> Result<(Value, RepairOutcome), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("input is empty or whitespace-only".into());
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok((v, RepairOutcome::Clean));
    }
    let repaired = repair_json(trimmed, &RepairOptions::default()).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&repaired).map_err(|e| e.to_string())?;
    Ok((value, RepairOutcome::Repaired))
}

/// Adapter convenience: parse raw tool-call `arguments` with two
/// fallback rules:
///
/// 1. **Empty / whitespace-only input** → `Value::Object({})`.
///    Some providers emit `arguments: ""` for tools with no
///    parameters; treating this as `{}` lets the schema validator
///    accept it on parameterless tools.
/// 2. **Non-empty but unrecoverable** → `Value::String(raw)`. The
///    model's original output survives intact for diagnostics
///    (logs) and is visible to downstream layers — schema validation's
///    schema validator surfaces `"expected object, got string"`
///    AND the raw bytes can be echoed back to the LLM if the agent
///    loop wants to reflect the malformed input verbatim.
///
/// **Coco-rs deviation**: some adapters substitute `{}` on failure so the
/// LLM gets a "missing required field" reply on the next turn; coco-rs
/// keeps the raw string so the validator + telemetry have full signal.
/// The trade-off favours diagnosability and model-side context: the raw
/// output is visible to whatever path builds the final
/// `<tool_use_error>` body.
///
/// Emits a `warn!` on both repair-assisted parses and total
/// failures so ops can monitor real-world repair frequency without
/// sampling code.
pub fn parse_tool_arguments_or_empty(raw: &str, tool_name: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    match parse_with_repair(raw) {
        Ok((v, RepairOutcome::Clean)) => v,
        Ok((v, RepairOutcome::Repaired)) => {
            tracing::warn!(
                target: "vercel_ai::tool_call",
                tool_name,
                args_bytes = raw.len(),
                "tool-call arguments JSON required repair before parse"
            );
            v
        }
        Err(err) => {
            tracing::warn!(
                target: "vercel_ai::tool_call",
                tool_name,
                args_bytes = raw.len(),
                error = %err,
                "tool-call arguments parse failed; preserving raw string for downstream diagnostics"
            );
            Value::String(raw.to_string())
        }
    }
}

#[cfg(test)]
#[path = "json_repair.test.rs"]
mod tests;
