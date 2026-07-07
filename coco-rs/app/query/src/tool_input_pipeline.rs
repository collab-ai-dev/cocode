//! Crate-private orchestration boundary for model-emitted tool input.
//!
//! This module intentionally lives in `app/query`: provider wire state,
//! observable transcript normalization, tool lookup, and proof-carrying
//! execution input meet here. `coco-tool-runtime` remains the execution
//! boundary through [`coco_tool_runtime::ValidatedInput`].

use std::sync::Arc;

use coco_inference::ToolInputWireState;
use coco_llm_types::ToolCallPart;
use coco_llm_types::ToolInputInvalidReason;
use coco_tool_runtime::SchemaIssue;
use coco_tool_runtime::ValidatedInput;
use coco_tool_runtime::traits::DynTool;
use serde_json::Value;

use crate::tool_input_normalizer::ToolInputNormalizationContext;

#[derive(Debug, Clone)]
pub(crate) struct ParsedToolInput {
    pub input: Value,
    pub invalid: bool,
    pub invalid_reason: Option<ToolInputInvalidReason>,
}

pub(crate) fn from_wire_state(
    tool_name: &str,
    input_state: &ToolInputWireState,
) -> ParsedToolInput {
    match input_state {
        ToolInputWireState::Empty => ParsedToolInput {
            input: Value::Object(Default::default()),
            invalid: false,
            invalid_reason: None,
        },
        ToolInputWireState::ParsedJson(value) => ParsedToolInput {
            input: value.clone(),
            invalid: false,
            invalid_reason: None,
        },
        ToolInputWireState::UnrecoverableRaw { raw, error } => ParsedToolInput {
            input: Value::String(raw.clone()),
            invalid: true,
            invalid_reason: Some(ToolInputInvalidReason::JsonParseFailed {
                raw: raw.clone(),
                error: error.clone(),
            }),
        },
        ToolInputWireState::RawStringAllowed { raw } => {
            tracing::warn!(
                target: "coco_query::tool_input",
                tool_name,
                args_bytes = raw.len(),
                "tool-call arguments preserved as raw string for tool-runtime coercion"
            );
            ParsedToolInput {
                input: Value::String(raw.clone()),
                invalid: false,
                invalid_reason: None,
            }
        }
    }
}

pub(crate) fn normalize_observable(
    tool_name: &str,
    input: Value,
    ctx: ToolInputNormalizationContext<'_>,
) -> Value {
    crate::tool_input_normalizer::normalize_observable_tool_input(tool_name, input, ctx)
}

pub(crate) fn validate_tool_call(
    tc: &mut ToolCallPart,
    tool: Option<&Arc<dyn DynTool>>,
) -> Option<ValidatedInput> {
    crate::tool_input_validate::validate_tool_call(tc, tool)
}

pub(crate) fn validate_updated_input(
    tool: &dyn DynTool,
    input: Value,
) -> Result<ValidatedInput, Vec<SchemaIssue>> {
    ValidatedInput::validate(tool, input)
}
