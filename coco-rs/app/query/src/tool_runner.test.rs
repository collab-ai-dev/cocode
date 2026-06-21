use std::sync::Arc;

use coco_llm_types::LlmMessage;
use coco_llm_types::ToolCallPart;
use coco_llm_types::ToolContentPart;
use coco_llm_types::ToolResultContent;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::ToolOverrides;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::prepare_committed_tool_call;
use crate::helpers::ToolCompletionEventMode;

fn registry_with(tool: Arc<dyn coco_tool_runtime::DynTool>) -> ToolRegistry {
    let registry = ToolRegistry::new();
    registry.register(tool);
    registry
}

fn tool_result_text(message: &Message) -> &str {
    let Message::ToolResult(result) = message else {
        panic!("expected tool result");
    };
    let LlmMessage::Tool { content, .. } = &result.message else {
        panic!("expected tool-role message");
    };
    let Some(ToolContentPart::ToolResult(result)) = content.first() else {
        panic!("expected tool result content");
    };
    match &result.output {
        ToolResultContent::Text { value, .. } | ToolResultContent::ErrorText { value, .. } => {
            value.as_str()
        }
        other => panic!("expected text output, got {other:?}"),
    }
}

#[tokio::test]
async fn deferred_tool_call_before_tool_search_does_not_schema_validate() {
    // WebFetch is a genuinely deferred, default-enabled tool. (Plan-mode tools
    // are no longer deferred, so they can't exercise the not-yet-loaded path.)
    let tools = registry_with(Arc::new(coco_tools::tools::WebFetchTool));
    let ctx = ToolUseContext::test_default()
        .with_model_capabilities(
            /*supports_tool_reference*/ false, /*supports_client_side_tool_search*/ true,
        )
        .with_tool_search_candidates(true);
    let mut history = MessageHistory::new();
    let tc = ToolCallPart::new(
        "call-deferred",
        "WebFetch",
        json!({"summary": "wrong shape"}),
    );

    let prepared = prepare_committed_tool_call(
        &None,
        &mut history,
        &tools,
        &ctx,
        &tc,
        ToolCompletionEventMode::Emit,
        None,
    )
    .await;

    assert!(prepared.is_none());
    assert_eq!(history.len(), 1);
    let text = tool_result_text(history.iter().next().unwrap());
    assert!(
        text.contains("deferred tool that has not been loaded yet"),
        "{text}"
    );
    assert!(text.contains("select:WebFetch"));
    assert!(!text.contains("InputValidationError"));
}

/// calm-bouncing-biscuit regression: a freeform apply_patch call arrives as
/// a BARE STRING. The prepared call must carry the coerced `{patch: …}`
/// object — never the raw string — so permission carve-outs and
/// execute-time `T::Input` deserialization see the typed shape, while the
/// committed `ToolCallPart` keeps the wire shape for the provider
/// round-trip.
#[tokio::test]
async fn test_prepare_committed_freeform_raw_string_threads_coerced_input() {
    let tools = registry_with(Arc::new(coco_tools::tools::ApplyPatchTool));
    let mut ctx = ToolUseContext::test_default();
    ctx.tool_overrides =
        Arc::new(ToolOverrides::default().with_extra(ToolId::Builtin(ToolName::ApplyPatch)));
    let mut history = MessageHistory::new();
    let raw = "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch\n";
    let tc = ToolCallPart::new("call-1", "apply_patch", json!(raw));

    let prepared = prepare_committed_tool_call(
        &None,
        &mut history,
        &tools,
        &ctx,
        &tc,
        ToolCompletionEventMode::Emit,
        None,
    )
    .await
    .expect("raw freeform input must prepare cleanly");

    assert_eq!(prepared.input.as_value(), &json!({ "patch": raw }));
    // Wire shape untouched: the assistant message round-trips the raw
    // envelope to the provider (`custom_tool_call.input` is a string).
    assert_eq!(tc.input, json!(raw));
    // No synthetic error tool_result was pushed.
    assert_eq!(history.len(), 0);
}

/// Double-encoded function arguments (`"{\"file_path\": …}"` as a JSON
/// string) take the string-recovery path; the recovered object must reach
/// the prepared call the same way freeform coercion does.
#[tokio::test]
async fn test_prepare_committed_double_encoded_json_threads_recovered_input() {
    let tools = registry_with(Arc::new(coco_tools::tools::ReadTool));
    let ctx = ToolUseContext::test_default();
    let mut history = MessageHistory::new();
    let tc = ToolCallPart::new(
        "call-2",
        "Read",
        json!("{\"file_path\": \"/tmp/recovered.txt\"}"),
    );

    let prepared = prepare_committed_tool_call(
        &None,
        &mut history,
        &tools,
        &ctx,
        &tc,
        ToolCompletionEventMode::Emit,
        None,
    )
    .await
    .expect("double-encoded input must recover and prepare");

    assert_eq!(
        prepared.input.as_value(),
        &json!({ "file_path": "/tmp/recovered.txt" })
    );
    assert_eq!(tc.input, json!("{\"file_path\": \"/tmp/recovered.txt\"}"));
}

#[test]
fn strip_internal_underscore_keys_removes_underscore_keys() {
    let mut input = json!({
        "command": "echo hi",
        "_simulatedSedEdit": { "filePath": "/tmp/x", "newContent": "PWNED" },
        "_other_internal": 1,
    });
    assert!(super::strip_internal_underscore_keys(&mut input));
    assert_eq!(input, json!({ "command": "echo hi" }));
}

#[test]
fn strip_internal_underscore_keys_noop_when_absent() {
    let mut input = json!({ "command": "ls" });
    assert!(!super::strip_internal_underscore_keys(&mut input));
    assert_eq!(input, json!({ "command": "ls" }));
}

#[test]
fn strip_internal_underscore_keys_noop_on_non_object() {
    let mut input = json!("raw string");
    assert!(!super::strip_internal_underscore_keys(&mut input));
    assert_eq!(input, json!("raw string"));
}
