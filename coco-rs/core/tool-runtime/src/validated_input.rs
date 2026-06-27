//! [`ValidatedInput`] — proof-carrying tool input.
//!
//! A tool call's wire input arrives in one of two shapes:
//!
//! - **Function tools** — a JSON object parsed from the provider's
//!   `arguments` string.
//! - **Freeform/custom tools** (OpenAI Responses `type:"custom"`, e.g.
//!   apply_patch) — a bare string the model emits under a grammar. The tool
//!   wraps it into the typed JSON its schema expects via
//!   [`DynTool::coerce_raw_string_input`] (apply_patch: `{"patch": raw}`).
//!
//! History/wire carriers (`ToolCallPart.input`) intentionally keep the raw
//! shape — the OpenAI Responses round-trip replays a custom tool call's input
//! as the original string. Everything from the preparation seam onward
//! (permission evaluation, hooks' `updated_input` re-validation, execution)
//! must instead see the coerced, schema-validated shape. `ValidatedInput` is
//! that seam as a type: the only constructor is [`ValidatedInput::validate`],
//! so holding a value is proof coercion + schema validation ran. Execution
//! carriers ([`crate::PendingToolCall`], `call_plan::PreparedToolCall`)
//! require it, which makes "raw freeform string reaches
//! `serde_json::from_value::<T::Input>` at execute time" unrepresentable.

use serde_json::Value;

use crate::schema::SchemaIssue;
use crate::traits::DynTool;

/// Tool input that has passed freeform coercion + runtime schema validation
/// for a specific tool. See the module docs for the seam this type pins.
#[derive(Debug, Clone)]
pub struct ValidatedInput(Value);

impl ValidatedInput {
    /// Coerce and validate `input` against `tool`'s runtime schema.
    ///
    /// A bare-string input is first offered to the tool's
    /// [`DynTool::coerce_raw_string_input`] (freeform tools wrap it into
    /// their typed JSON; function tools return `None` and the string falls
    /// through to schema validation, which reports the type mismatch).
    pub fn validate(tool: &dyn DynTool, input: Value) -> Result<Self, Vec<SchemaIssue>> {
        let input = match input {
            Value::String(raw) => match tool.coerce_raw_string_input(&raw) {
                Some(coerced) => coerced,
                None => Value::String(raw),
            },
            other => other,
        };
        tool.runtime_validation_schema().validate(&input)?;
        Ok(Self(input))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

#[cfg(test)]
#[path = "validated_input.test.rs"]
mod tests;
