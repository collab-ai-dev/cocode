//! LLM-output JSON repair, single source of truth.
//!
//! Wraps the [`llm_json`] crate (Rust port of the Python `json_repair`
//! library) with a coco-friendly API that distinguishes clean parses
//! from repair-assisted ones — useful for telemetry.
//!
//! # When to use
//!
//! **Use** when parsing JSON emitted by an LLM that strict
//! [`serde_json`] rejects: trailing commas, missing brackets, single
//! quotes around keys/values, markdown code fences, smart quotes,
//! truncated output. Common at the boundaries where models emit
//! structured data:
//! - Streaming tool-input accumulation (parse at `ToolInputEnd`, not
//!   per-delta — see policy note below)
//! - Caller-side parsing of `response_format: json_schema` text from
//!   models that don't fully respect constrained decoding
//!
//! **Do NOT use** during streaming accumulation. The repairer expects
//! complete-ish input; calling it on a half-arrived `{"a":1,` will
//! silently "close" the structure into wrong content. Always defer to
//! the terminal event (`ToolInputEnd` / `ToolCall` / end-of-text).
//!
//! # Behavior is intentionally aggressive
//!
//! [`llm_json`] applies the full Python `json_repair` rule set:
//! single-quote to double-quote conversion, Python literal mapping
//! (`None` / `True` / `False`), markdown fence stripping, missing-
//! comma inference, truncation completion, etc. This **changes model
//! output semantics** in some cases. The repair fires only when strict
//! parse fails, and callers that consume the result still pass through
//! their own validation (permission gates, schema validators) — so
//! a silently-repaired Bash command still hits the safety net before
//! execution.
//!
//! Callers that need to monitor repair frequency consume the
//! [`RepairOutcome`] return value and emit a telemetry event when it
//! is [`RepairOutcome::Repaired`].

use serde_json::Value;

/// Outcome of a parse attempt — tags whether repair was necessary so
/// callers can record telemetry without re-parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Strict [`serde_json::from_str`] succeeded; no repair fired.
    Clean,
    /// Required one or more repair passes to parse. The original input
    /// was malformed against strict JSON syntax.
    Repaired,
}

/// Repair failed — input was so malformed neither strict parsing nor
/// the [`llm_json`] repairer could produce a valid value.
#[derive(Debug, thiserror::Error)]
pub enum JsonRepairError {
    /// Input was empty or whitespace-only. Distinguished from
    /// [`Self::Repair`] so callers can apply per-domain defaults
    /// (e.g. tool-input parsing returns `{}` for an empty buffer
    /// because the engine writes an empty string when the model
    /// emits no `ToolInputDelta` events for a parameterless call).
    #[error("input is empty or whitespace-only")]
    EmptyInput,
    /// Repair attempt produced a string that still doesn't parse.
    #[error("repaired text still failed serde_json parse: {0}")]
    Postparse(#[source] serde_json::Error),
    /// [`llm_json::repair_json`] itself returned an error.
    #[error("json repair failed: {0}")]
    Repair(String),
}

/// Parse JSON, attempting [`llm_json`] repair on strict-parse failure.
///
/// Two-stage:
/// 1. Fast path: [`serde_json::from_str`] on the trimmed input. Cheap
///    and never re-interprets correct JSON.
/// 2. Repair path: hand the trimmed input to [`llm_json::repair_json`]
///    with default options, then [`serde_json::from_str`] the result.
///
/// Returns the parsed [`Value`] plus a [`RepairOutcome`] tag so callers
/// can emit a `repair_event` telemetry record on the
/// [`RepairOutcome::Repaired`] branch.
///
/// Empty / whitespace-only input is [`JsonRepairError::Postparse`]
/// (strict-empty is also a parse error in [`serde_json`]). Callers who
/// want `""` → `Value::Object({})` semantics should special-case
/// upstream (e.g. tool-input accumulation treats an empty buffer as
/// an empty object).
pub fn parse_with_repair(raw: &str) -> Result<(Value, RepairOutcome), JsonRepairError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(JsonRepairError::EmptyInput);
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok((v, RepairOutcome::Clean));
    }
    let repaired = llm_json::repair_json(trimmed, &llm_json::RepairOptions::default())
        .map_err(|e| JsonRepairError::Repair(e.to_string()))?;
    let value = serde_json::from_str::<Value>(&repaired).map_err(JsonRepairError::Postparse)?;
    Ok((value, RepairOutcome::Repaired))
}

/// Repair JSON, returning the (possibly fixed) text without parsing.
///
/// Useful when the caller wants to log/cache the repaired form, or
/// hand the string to a downstream system that does its own parse.
///
/// `Clean` outcome returns the trimmed input verbatim; `Repaired`
/// returns the [`llm_json::repair_json`] result.
pub fn repair_to_string(raw: &str) -> Result<(String, RepairOutcome), JsonRepairError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(JsonRepairError::EmptyInput);
    }
    if serde_json::from_str::<Value>(trimmed).is_ok() {
        return Ok((trimmed.to_string(), RepairOutcome::Clean));
    }
    let repaired = llm_json::repair_json(trimmed, &llm_json::RepairOptions::default())
        .map_err(|e| JsonRepairError::Repair(e.to_string()))?;
    Ok((repaired, RepairOutcome::Repaired))
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
