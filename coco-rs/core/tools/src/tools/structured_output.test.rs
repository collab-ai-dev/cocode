use super::StructuredOutputTool;
use coco_messages::AttachmentBody;
use coco_messages::Message;
use coco_messages::SilentPayload;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::context::ToolUseContext;
use coco_types::AttachmentKind;
use coco_types::ToolId;
use coco_types::ToolName;
use pretty_assertions::assert_eq;
use serde_json::json;
// `DynTool` is imported per-test where needed — pulling it into module
// scope would create method-resolution ambiguity between `Tool::execute`
// (typed) and `DynTool::execute` (Value) for the `Self::Input = Value`
// case below.

fn person_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age":  { "type": "integer", "minimum": 0 }
        },
        "required": ["name"],
        "additionalProperties": false,
    })
}

#[test]
fn new_rejects_invalid_schema() {
    let bad = json!({ "type": "not-a-real-type" });
    let err = StructuredOutputTool::new(bad).unwrap_err();
    assert!(err.contains("invalid JSON schema"), "got: {err}");
}

#[test]
fn id_and_name_use_structured_output_wire_form() {
    let tool = StructuredOutputTool::new(person_schema()).unwrap();
    assert_eq!(tool.id(), ToolId::Builtin(ToolName::StructuredOutput));
    assert_eq!(tool.name(), "StructuredOutput");
}

#[test]
fn input_schema_forwards_user_supplied_properties_and_required() {
    let tool = StructuredOutputTool::new(person_schema()).unwrap();
    let schema = tool.runtime_validation_schema().as_value();
    assert!(schema["properties"].get("name").is_some());
    assert!(schema["properties"].get("age").is_some());
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(required, vec!["name"]);
}

// ---------------------------------------------------------------------------
// Schema wire-envelope preservation — guards against the DeepSeek
// `type: null` regression. `input_json_schema()` must return the
// user-supplied schema verbatim instead of falling through to the
// blanket `derive_input_schema_value::<Value>()` default. Mirrors the
// equivalent `McpTool` regression tests.
// ---------------------------------------------------------------------------

#[test]
fn input_json_schema_returns_user_supplied_schema_verbatim() {
    use coco_tool_runtime::DynTool;
    let schema = person_schema();
    let tool = StructuredOutputTool::new(schema.clone()).unwrap();
    let tool: &dyn DynTool = &tool;
    assert_eq!(tool.runtime_validation_schema().as_value(), &schema);
}

#[test]
fn input_json_schema_preserves_top_level_fields_beyond_properties_and_required() {
    // `input_schema()` only forwards `properties` + `required`; the full
    // JSON Schema envelope (additionalProperties, $schema, descriptions,
    // etc.) must round-trip through `input_json_schema()` so strict
    // providers see exactly what the user wrote.
    use coco_tool_runtime::DynTool;
    let schema = json!({
        "type": "object",
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Person",
        "description": "A person record",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"],
        "additionalProperties": false
    });
    let tool = StructuredOutputTool::new(schema.clone()).unwrap();
    let tool: &dyn DynTool = &tool;
    let echoed = tool.runtime_validation_schema().as_value();
    assert_eq!(echoed, &schema);
}

#[test]
fn input_json_schema_does_not_emit_type_null_for_value_input() {
    // Regression: with `type Input = Value` and no override, the blanket
    // default produces a schema whose top-level `type` is absent or
    // non-string — strict OpenAI-compatible providers reject it as
    // `type: null`. With the override in place, the top-level `type`
    // must be a string equal to "object" (whatever the user wrote).
    use coco_tool_runtime::DynTool;
    let tool = StructuredOutputTool::new(person_schema()).unwrap();
    let tool: &dyn DynTool = &tool;
    let schema = tool.runtime_validation_schema().as_value();
    assert_eq!(
        schema.get("type").and_then(|v| v.as_str()),
        Some("object"),
        "top-level `type` must round-trip from user schema; got: {schema}"
    );
}

#[tokio::test]
async fn execute_valid_input_emits_silent_structured_output_attachment() {
    let tool = StructuredOutputTool::new(person_schema()).unwrap();
    let ctx = ToolUseContext::test_default();
    let payload = json!({ "name": "Alice", "age": 30 });
    let result = tool
        .execute(payload.clone(), &ctx)
        .await
        .expect("schema-conforming input must succeed");
    assert_eq!(result.data, "Structured output provided successfully");
    assert_eq!(result.new_messages.len(), 1);
    let Message::Attachment(att) = &result.new_messages[0] else {
        panic!("expected attachment, got {:?}", result.new_messages[0]);
    };
    assert_eq!(att.kind, AttachmentKind::StructuredOutput);
    let AttachmentBody::Silent(SilentPayload::StructuredOutput(p)) = &att.body else {
        panic!("expected SilentPayload::StructuredOutput");
    };
    assert_eq!(p.data, payload);
    // Side-channel accessor mirrors the same data so the engine pipeline
    // can lift it without re-walking new_messages.
    assert_eq!(result.structured_output(), Some(payload));
}

#[tokio::test]
async fn execute_invalid_input_returns_execution_failed_with_schema_path() {
    let tool = StructuredOutputTool::new(person_schema()).unwrap();
    let ctx = ToolUseContext::test_default();
    // missing required `name`
    let err = tool
        .execute(json!({ "age": 30 }), &ctx)
        .await
        .expect_err("missing required field must error");
    let ToolError::ExecutionFailed { message, .. } = err else {
        panic!("expected ToolError::ExecutionFailed");
    };
    assert!(
        message.starts_with("Output does not match required schema:"),
        "unexpected message: {message}"
    );
}

#[tokio::test]
async fn execute_additional_property_rejected_when_schema_disallows() {
    let tool = StructuredOutputTool::new(person_schema()).unwrap();
    let ctx = ToolUseContext::test_default();
    let err = tool
        .execute(json!({ "name": "Alice", "extra": true }), &ctx)
        .await
        .expect_err("additionalProperties: false must reject `extra`");
    let ToolError::ExecutionFailed { message, .. } = err else {
        panic!("expected ToolError::ExecutionFailed");
    };
    assert!(
        message.contains("Output does not match required schema"),
        "unexpected message: {message}"
    );
}
