use std::sync::Arc;

use coco_llm_types::ToolCallPart;
use coco_messages::MessageHistory;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolCallErrorKind;
use coco_tool_runtime::ToolLookup;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::CoreEvent;
use coco_types::ToolId;
use coco_types::ToolName;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

use crate::emit::emit_stream;
use crate::helpers::ToolCompletionEventMode;
use crate::helpers::complete_tool_call_with_error_mode;

/// Resolved and validated tool call ready for permission/hook/execution.
///
/// `input` is the coerced, schema-validated form of the wire input
/// (freeform raw strings wrapped via `coerce_raw_string_input`,
/// double-encoded JSON recovered). The original `ToolCallPart.input` keeps
/// the wire shape for history/provider round-trips; everything downstream
/// of preparation — permission evaluation, hooks, execution — must consume
/// this value instead.
pub(crate) struct PreparedToolCall {
    pub tool_id: ToolId,
    pub tool: Arc<dyn DynTool>,
    pub input: coco_tool_runtime::ValidatedInput,
}

/// Prepare one committed assistant tool call.
///
/// This owns the first part of the tool-result pairing invariant:
/// every committed call emits `ToolUseQueued`; calls that cannot become
/// runnable because the tool is unknown or the input is invalid are completed
/// here with exactly one model-visible error result.
///
/// `tool_call.input` is already the observable input: both the streaming
/// and non-streaming engine paths run
/// `tool_input_normalizer::normalize_observable_tool_input` while building
/// the assistant-message `ToolCallPart` this function receives, so no
/// re-normalization happens here.
pub(crate) async fn prepare_committed_tool_call(
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
    history: &mut MessageHistory,
    tools: &ToolRegistry,
    ctx: &ToolUseContext,
    tool_call: &ToolCallPart,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
) -> Option<PreparedToolCall> {
    let mut deferred_tool_completions = deferred_tool_completions;
    let tool_id: ToolId = tool_call
        .tool_name
        .parse()
        .unwrap_or_else(|_| ToolId::Custom(tool_call.tool_name.clone()));

    let _delivered = emit_stream(
        event_tx,
        crate::AgentStreamEvent::ToolUseQueued {
            call_id: tool_call.tool_call_id.clone(),
            name: tool_call.tool_name.clone(),
            input: tool_call.input.clone(),
        },
    )
    .await;

    let tool = match tools.lookup_loaded(&tool_id, ctx) {
        ToolLookup::Loaded(tool) => tool,
        ToolLookup::Deferred { name } => {
            warn!(
                tool = tool_call.tool_name,
                resolved_tool = name,
                "deferred tool called before ToolSearch discovery"
            );
            let output = format!(
                "<tool_use_error>No such tool available: {}. It is a deferred tool that has not been loaded yet; use ToolSearch with query \"select:{}\" first.</tool_use_error>",
                tool_call.tool_name, name
            );
            complete_tool_call_with_error_mode(
                event_tx,
                history,
                &tool_call.tool_call_id,
                &tool_call.tool_name,
                &tool_id,
                &output,
                ToolCallErrorKind::UnknownTool,
                completion_event_mode,
                deferred_tool_completions.take(),
            )
            .await;
            return None;
        }
        ToolLookup::Unavailable => {
            warn!(
                tool = tool_call.tool_name,
                "tool not available in current context"
            );
            // Mirror error wrap's `<tool_use_error>No such tool available: ...>`
            // wrap so the model sees the same format whether the
            // unknown-tool branch fires here (registry miss) or in
            // `tool_call_preparer` (schema validation catch for hallucinated names
            // not in the per-call tools list).
            let output = format!(
                "<tool_use_error>No such tool available: {}</tool_use_error>",
                tool_call.tool_name
            );
            complete_tool_call_with_error_mode(
                event_tx,
                history,
                &tool_call.tool_call_id,
                &tool_call.tool_name,
                &tool_id,
                &output,
                ToolCallErrorKind::UnknownTool,
                completion_event_mode,
                deferred_tool_completions.take(),
            )
            .await;
            return None;
        }
    };

    // wire parsing + schema validation short-circuit. The provider adapter (wire parsing)
    // may have flagged the call as `invalid` when raw `arguments`
    // bytes were unrecoverable. schema validation runs only
    // when wire parsing left the call unflagged; otherwise we preserve
    // wire parsing's reason. Both paths converge on the same `<tool_use_error>`
    // wrap selection so the model sees one format whether the failure
    // originated on the wire or in the schema validator.
    //
    // `validate_tool_call` mutates only this clone's invalid flags; the
    // committed `tool_call` keeps its wire-shape input for the provider
    // round-trip. The coerced, schema-validated input it returns is the
    // value every downstream consumer (permission evaluation, hooks,
    // execution) sees — threading it through `PreparedToolCall` is what
    // keeps the serde-backed validators and `T::Input` deserialization
    // from ever meeting a raw freeform string.
    let mut validated = tool_call.clone();
    let validated_input =
        crate::tool_input_validate::validate_tool_call(&mut validated, Some(&tool));
    let Some(mut validated_input) = validated_input else {
        let message = match validated.invalid_reason {
            Some(coco_llm_types::ToolInputInvalidReason::SchemaViolation { message }) => {
                format!("<tool_use_error>InputValidationError: {message}</tool_use_error>")
            }
            Some(coco_llm_types::ToolInputInvalidReason::NoSuchTool { tool_name }) => {
                format!("<tool_use_error>No such tool available: {tool_name}</tool_use_error>")
            }
            Some(coco_llm_types::ToolInputInvalidReason::JsonParseFailed { error, .. }) => {
                format!(
                    "<tool_use_error>The tool call arguments could not be parsed as JSON: {error}. \
                     Please retry with valid JSON.</tool_use_error>"
                )
            }
            None => "<tool_use_error>Invalid tool call</tool_use_error>".to_string(),
        };
        complete_tool_call_with_error_mode(
            event_tx,
            history,
            &tool_call.tool_call_id,
            &tool_call.tool_name,
            &tool_id,
            &message,
            ToolCallErrorKind::SchemaFailed,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
        return None;
    };

    // Defense-in-depth: drop model-injected internal `_`-prefixed fields
    // (e.g. Bash `_simulatedSedEdit`) from the COERCED input. They are
    // reserved for trusted UI dialogs and hidden from the model-facing
    // spec, but the runtime schema accepts them — so without this a model
    // could smuggle one (including via raw-string `arguments` that decode
    // into an object here) to BashTool's internal short-circuit: an
    // arbitrary Edit-style write. Stripping post-coercion covers both
    // object and string-encoded arguments. Re-validation cannot fail —
    // removing an optional field keeps the input schema-valid.
    if tool_call.tool_name == ToolName::Bash.as_str() {
        let mut value = validated_input.as_value().clone();
        if strip_internal_underscore_keys(&mut value)
            && let Ok(revalidated) =
                coco_tool_runtime::ValidatedInput::validate(tool.as_ref(), value)
        {
            validated_input = revalidated;
        }
    }

    // Validate the coerced input, not the raw `tool_call.input` — feeding
    // a freeform tool's raw string to the serde-backed `validate_input`
    // would fail with `invalid type: string`.
    let validation = tool.validate_input(validated_input.as_value(), ctx);
    if !validation.is_valid() {
        let message = match validation {
            coco_tool_runtime::ValidationResult::Invalid { message, .. } => {
                format!("Invalid input: {message}")
            }
            coco_tool_runtime::ValidationResult::Valid => "Invalid input".to_string(),
        };
        warn!(
            tool = tool_call.tool_name,
            tool_use_id = tool_call.tool_call_id,
            %message,
            "tool input validation failed"
        );
        complete_tool_call_with_error_mode(
            event_tx,
            history,
            &tool_call.tool_call_id,
            &tool_call.tool_name,
            &tool_id,
            &message,
            ToolCallErrorKind::ValidationFailed,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
        return None;
    }

    Some(PreparedToolCall {
        tool_id,
        tool,
        input: validated_input,
    })
}

/// Remove every `_`-prefixed key from a tool-input object, returning whether
/// any key was removed. A defense-in-depth strip for internal fields (e.g.
/// Bash `_simulatedSedEdit`) that trusted UI dialogs populate but the model
/// must never supply. No-op (returns `false`) on non-object inputs.
fn strip_internal_underscore_keys(input: &mut Value) -> bool {
    let Some(obj) = input.as_object_mut() else {
        return false;
    };
    let internal_keys: Vec<String> = obj.keys().filter(|k| k.starts_with('_')).cloned().collect();
    let removed = !internal_keys.is_empty();
    for key in internal_keys {
        obj.remove(&key);
    }
    removed
}

#[cfg(test)]
#[path = "tool_runner.test.rs"]
mod tests;
