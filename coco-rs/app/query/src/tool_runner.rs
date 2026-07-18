use std::sync::Arc;

use coco_llm_types::ToolCallPart;
use coco_messages::MessageHistory;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::MaterializedToolLookup;
use coco_tool_runtime::ToolCallErrorKind;
use coco_tool_runtime::ToolMaterialization;
use coco_tool_runtime::ToolPlacement;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::CoreEvent;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::WireToolName;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::warn;

use crate::emit::emit_stream;
use crate::helpers::ToolCompletionEventMode;
use crate::helpers::complete_tool_call_with_error_mode;

pub(crate) struct ToolSettlement<'a> {
    pub registry: &'a ToolRegistry,
    pub materialization: &'a ToolMaterialization,
}

/// Resolved and validated tool call ready for permission/hook/execution.
///
/// `input` is the coerced, schema-validated form of the wire input
/// (freeform raw strings wrapped via `coerce_raw_string_input`,
/// double-encoded JSON recovered). The original `ToolCallPart.input` keeps
/// the wire shape for history/provider round-trips; everything downstream
/// of preparation — permission evaluation, hooks, execution — must consume
/// this value instead.
pub(crate) struct ResolvedToolCall {
    pub tool_id: ToolId,
    pub tool: Arc<dyn DynTool>,
    pub input: coco_tool_runtime::ValidatedInput,
    /// Canonical semantic call consumed by hooks, permissions and audit.
    pub semantic_call: ToolCallPart,
    /// Provider call name used only when constructing the paired tool result.
    pub provider_tool_name: WireToolName,
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
    settlement: ToolSettlement<'_>,
    ctx: &ToolUseContext,
    tool_call: &ToolCallPart,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
) -> Option<ResolvedToolCall> {
    let mut deferred_tool_completions = deferred_tool_completions;
    let unknown_tool_id = ToolId::Custom(tool_call.tool_name.clone());

    let _delivered = emit_stream(
        event_tx,
        crate::AgentStreamEvent::ToolUseQueued {
            call_id: tool_call.tool_call_id.clone(),
            name: tool_call.tool_name.clone(),
            input: tool_call.input.clone(),
        },
    )
    .await;

    // Carrier calls (`use_tool { name, arguments }`) resolve to their real
    // target here, before the normal lookup/validation. The wire identity
    // (`use_tool` + call id) is preserved by the caller's `ToolResultContext`
    // for provider result pairing; the returned `ResolvedToolCall` carries the
    // resolved target so permissions/hooks/execution key on the real tool.
    if tool_call.tool_name == ToolName::UseTool.as_str() {
        return prepare_use_tool_call(
            event_tx,
            history,
            &settlement,
            ctx,
            tool_call,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
    }

    let (tool_id, tool, provider_tool_name) = match settlement
        .materialization
        .lookup_wire(settlement.registry, &tool_call.tool_name)
    {
        MaterializedToolLookup::Loaded(materialized) => (
            materialized.tool_id,
            materialized.tool,
            materialized.wire_name,
        ),
        MaterializedToolLookup::Deferred { name, tool: _ } => {
            warn!(
                tool = tool_call.tool_name,
                resolved_tool = name,
                "deferred tool called before ToolSearch discovery"
            );
            let output = format!(
                "<tool_use_error>No such tool available: {}. It is deferred; use ToolSearch with query \"select:{}\" to obtain its bounded schema first.</tool_use_error>",
                tool_call.tool_name, name
            );
            complete_tool_call_with_error_mode(
                event_tx,
                history,
                &tool_call.tool_call_id,
                &tool_call.tool_name,
                &unknown_tool_id,
                &output,
                ToolCallErrorKind::UnknownTool,
                completion_event_mode,
                deferred_tool_completions.take(),
            )
            .await;
            return None;
        }
        MaterializedToolLookup::Stale { name } => {
            warn!(
                tool = tool_call.tool_name,
                resolved_tool = name,
                "tool registration changed after provider-turn materialization"
            );
            let output = format!(
                "<tool_use_error>No such tool available: {}. Its registration changed after this turn's tool list was sent; retry the request so the current tool list can be used.</tool_use_error>",
                tool_call.tool_name
            );
            complete_tool_call_with_error_mode(
                event_tx,
                history,
                &tool_call.tool_call_id,
                &tool_call.tool_name,
                &unknown_tool_id,
                &output,
                ToolCallErrorKind::UnknownTool,
                completion_event_mode,
                deferred_tool_completions.take(),
            )
            .await;
            return None;
        }
        MaterializedToolLookup::Unavailable => {
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
                &unknown_tool_id,
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
    // execution) sees — threading it through `ResolvedToolCall` is what
    // keeps the serde-backed validators and `T::Input` deserialization
    // from ever meeting a raw freeform string.
    let mut validated = tool_call.clone();
    let validated_input =
        crate::tool_input_pipeline::validate_tool_call(&mut validated, Some(&tool));
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
                crate::tool_input_pipeline::validate_updated_input(tool.as_ref(), value)
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

    let mut semantic_call = tool_call.clone();
    semantic_call.tool_name = tool_id.to_string();
    Some(ResolvedToolCall {
        tool_id,
        tool,
        input: validated_input,
        semantic_call,
        provider_tool_name,
    })
}

/// Resolve a `use_tool` carrier call to its real target.
///
/// The provider wire call keeps the `use_tool` name + call id — the caller's
/// `ToolResultContext` pairs the result on it (Google pairs a function response
/// by name) — while the returned [`ResolvedToolCall`] carries the resolved
/// TARGET so validation, permissions, hooks, and execution all key on the real
/// tool. Every failure completes exactly one model-visible error under the WIRE
/// identity, preserving the tool-result pairing invariant.
///
/// Only targets materialized as [`ToolPlacement::UseTool`] can use this path;
/// loaded and deferred targets return a placement-specific steering error.
async fn prepare_use_tool_call(
    event_tx: &Option<mpsc::Sender<CoreEvent>>,
    history: &mut MessageHistory,
    settlement: &ToolSettlement<'_>,
    ctx: &ToolUseContext,
    tool_call: &ToolCallPart,
    completion_event_mode: ToolCompletionEventMode,
    deferred_tool_completions: Option<&mut crate::helpers::DeferredToolCompletionBuffer>,
) -> Option<ResolvedToolCall> {
    let mut deferred_tool_completions = deferred_tool_completions;
    let wire_id = &tool_call.tool_call_id;
    let wire_name = &tool_call.tool_name; // always "use_tool"
    let carrier_id = ToolId::Builtin(ToolName::UseTool);

    // 1. Parse the carrier `{ name, arguments }`. Parsed inline: app/query must
    //    not depend on coco-tools, where `UseToolInput` lives.
    let target_name = match tool_call.input.get("name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => {
            let msg = "<tool_use_error>use_tool requires a non-empty `name` naming the tool to \
                 invoke, exactly as ToolSearch returned it.</tool_use_error>"
                .to_string();
            complete_tool_call_with_error_mode(
                event_tx,
                history,
                wire_id,
                wire_name,
                &carrier_id,
                &msg,
                ToolCallErrorKind::ValidationFailed,
                completion_event_mode,
                deferred_tool_completions.take(),
            )
            .await;
            return None;
        }
    };
    let arguments = tool_call
        .input
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Null);

    // 2. Resolve the target by wire name (registry owns the map; no parsing)
    //    and apply placement-aware steering. Clone the fields we need so the
    //    materialization borrow ends before the registry staleness check.
    let (target_tool, target_id, target_canonical, target_reg, steering) =
        match settlement.materialization.lookup_by_wire_name(&target_name) {
            None => {
                let msg = format!(
                    "<tool_use_error>No such tool available: {target_name}</tool_use_error>"
                );
                complete_tool_call_with_error_mode(
                    event_tx,
                    history,
                    wire_id,
                    wire_name,
                    &carrier_id,
                    &msg,
                    ToolCallErrorKind::UnknownTool,
                    completion_event_mode,
                    deferred_tool_completions.take(),
                )
                .await;
                return None;
            }
            Some(t) => {
                let steering = match t.placement {
                    ToolPlacement::UseTool => None,
                    ToolPlacement::Loaded => Some(format!(
                        "<tool_use_error>{target_name} is already in your tool list — call it \
                         directly, not through use_tool.</tool_use_error>"
                    )),
                    ToolPlacement::Deferred => Some(format!(
                        "<tool_use_error>{target_name} is not loaded yet — use ToolSearch with \
                         query \"select:{target_name}\" first, then call it.</tool_use_error>"
                    )),
                };
                (
                    t.tool.clone(),
                    t.tool_id.clone(),
                    t.canonical_name.clone(),
                    t.registration_id,
                    steering,
                )
            }
        };
    if let Some(msg) = steering {
        complete_tool_call_with_error_mode(
            event_tx,
            history,
            wire_id,
            wire_name,
            &carrier_id,
            &msg,
            ToolCallErrorKind::UnknownTool,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
        return None;
    }

    // 3. Stale check against the live registry — fail closed on a
    //    replaced/removed registration.
    if settlement
        .registry
        .current_registration_id(&target_canonical)
        != Some(target_reg)
    {
        let msg = format!(
            "<tool_use_error>No such tool available: {target_name}. Its registration changed \
             after this turn's tool list was sent; retry the request.</tool_use_error>"
        );
        complete_tool_call_with_error_mode(
            event_tx,
            history,
            wire_id,
            wire_name,
            &carrier_id,
            &msg,
            ToolCallErrorKind::UnknownTool,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
        return None;
    }

    // 4. Validate `arguments` against the TARGET schema via the normal
    //    pipeline. Error results keep the WIRE identity so pairing holds.
    let mut synthetic = tool_call.clone();
    synthetic.tool_name = target_id.to_string();
    synthetic.input = arguments;
    let Some(validated_input) =
        crate::tool_input_pipeline::validate_tool_call(&mut synthetic, Some(&target_tool))
    else {
        let message = match synthetic.invalid_reason {
            Some(coco_llm_types::ToolInputInvalidReason::SchemaViolation { message }) => {
                format!("<tool_use_error>InputValidationError: {message}</tool_use_error>")
            }
            Some(coco_llm_types::ToolInputInvalidReason::NoSuchTool { tool_name }) => {
                format!("<tool_use_error>No such tool available: {tool_name}</tool_use_error>")
            }
            Some(coco_llm_types::ToolInputInvalidReason::JsonParseFailed { error, .. }) => {
                format!(
                    "<tool_use_error>The target tool's arguments could not be parsed as JSON: \
                     {error}. Please retry with valid JSON.</tool_use_error>"
                )
            }
            None => "<tool_use_error>Invalid tool call</tool_use_error>".to_string(),
        };
        complete_tool_call_with_error_mode(
            event_tx,
            history,
            wire_id,
            wire_name,
            &target_id,
            &message,
            ToolCallErrorKind::SchemaFailed,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
        return None;
    };
    let validation = target_tool.validate_input(validated_input.as_value(), ctx);
    if !validation.is_valid() {
        let message = match validation {
            coco_tool_runtime::ValidationResult::Invalid { message, .. } => {
                format!("Invalid input: {message}")
            }
            coco_tool_runtime::ValidationResult::Valid => "Invalid input".to_string(),
        };
        complete_tool_call_with_error_mode(
            event_tx,
            history,
            wire_id,
            wire_name,
            &target_id,
            &message,
            ToolCallErrorKind::ValidationFailed,
            completion_event_mode,
            deferred_tool_completions.take(),
        )
        .await;
        return None;
    }

    Some(ResolvedToolCall {
        tool_id: target_id,
        tool: target_tool,
        input: validated_input,
        semantic_call: synthetic,
        provider_tool_name: WireToolName::for_tool_id(&carrier_id),
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
