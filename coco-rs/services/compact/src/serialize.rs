//! Strategy → wire-format serializer.
//!
//! Converts the typed `[ContextEditStrategy]` list into the
//! camelCase JSON shape that `vercel-ai-anthropic` expects. The
//! Anthropic provider's
//! `transform_context_management` then snake-cases the keys for the
//! actual API request — we hand off camelCase so a single
//! transform site can keep the wire shape coherent.
//!
//! Other providers do not consume `context_management`: their
//! capability shim (see `coco_inference::capability`) returns
//! `false` and the encoder is never called.

use serde_json::Value;
use serde_json::json;

use crate::types::ClearToolInputs;
use crate::types::ContextEditStrategy;
use crate::types::ThinkingKeep;

/// Encode a strategy list as `{ edits: [...] }` ready to drop into
/// `AnthropicProviderOptions.context_management`. Returns `None` when
/// the input is empty so callers can omit the field entirely.
#[must_use]
pub fn encode_anthropic_context_management(strategies: &[ContextEditStrategy]) -> Option<Value> {
    if strategies.is_empty() {
        return None;
    }
    let edits: Vec<Value> = strategies.iter().map(strategy_to_value).collect();
    Some(json!({ "edits": edits }))
}

fn strategy_to_value(s: &ContextEditStrategy) -> Value {
    match s {
        ContextEditStrategy::ClearToolUses {
            trigger,
            keep_recent,
            clear_at_least,
            clear_inputs,
            exclude_tools,
            exclude_tool_strs,
        } => {
            let mut v = json!({ "type": "clear_tool_uses_20250919" });
            if let Some(t) = trigger {
                v["trigger"] = json!({ "type": "input_tokens", "value": t });
            }
            if let Some(keep) = keep_recent {
                v["keep"] = json!({ "type": "tool_uses", "value": keep.value });
            }
            if let Some(at_least) = clear_at_least {
                v["clearAtLeast"] = json!({ "type": "input_tokens", "value": at_least });
            }
            v["clearToolInputs"] = match clear_inputs {
                ClearToolInputs::All => Value::Bool(true),
                ClearToolInputs::None => Value::Bool(false),
                ClearToolInputs::SpecificTools(tools) => Value::Array(
                    tools
                        .iter()
                        .map(|t| Value::String(t.as_str().to_string()))
                        .collect(),
                ),
            };
            if !exclude_tools.is_empty() || !exclude_tool_strs.is_empty() {
                let mut excluded: Vec<Value> = exclude_tools
                    .iter()
                    .map(|t| Value::String(t.as_str().to_string()))
                    .collect();
                for s in exclude_tool_strs {
                    excluded.push(Value::String(s.clone()));
                }
                v["excludeTools"] = Value::Array(excluded);
            }
            v
        }
        ContextEditStrategy::ClearThinking { keep } => {
            let keep_val = match keep {
                ThinkingKeep::All => Value::String("all".to_string()),
                ThinkingKeep::Recent { turns } => {
                    json!({ "type": "thinking_turns", "value": turns })
                }
            };
            json!({ "type": "clear_thinking_20251015", "keep": keep_val })
        }
    }
}

#[cfg(test)]
#[path = "serialize.test.rs"]
mod tests;
