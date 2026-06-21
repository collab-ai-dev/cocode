use std::sync::Arc;

use coco_messages::ToolResult;
use coco_types::ToolId;
use pretty_assertions::assert_eq;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use super::ValidatedInput;
use crate::context::ToolUseContext;
use crate::error::ToolError;
use crate::traits::DescriptionOptions;
use crate::traits::DynTool;
use crate::traits::Tool;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PatchInput {
    patch: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct EmptyOutput {}

/// Freeform tool double: coerces a bare string into `{patch}` the same way
/// apply_patch does. `coerces: false` turns it into a plain function tool.
struct PatchTool {
    coerces: bool,
}

#[async_trait::async_trait]
impl Tool for PatchTool {
    type Input = PatchInput;
    type Output = EmptyOutput;

    fn id(&self) -> ToolId {
        ToolId::Custom("patch_test".into())
    }

    fn name(&self) -> &str {
        "patch_test"
    }

    crate::impl_runtime_schema!(PatchInput);

    fn description(&self, _input: &PatchInput, _options: &DescriptionOptions) -> String {
        String::new()
    }

    async fn prompt(&self, _options: &crate::traits::PromptOptions) -> String {
        String::new()
    }

    fn coerce_raw_string_input(&self, raw: &str) -> Option<Value> {
        self.coerces.then(|| json!({ "patch": raw }))
    }

    async fn execute(
        &self,
        _input: PatchInput,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<EmptyOutput>, ToolError> {
        Ok(ToolResult {
            data: EmptyOutput {},
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

fn freeform_tool() -> Arc<dyn DynTool> {
    Arc::new(PatchTool { coerces: true })
}

fn function_tool() -> Arc<dyn DynTool> {
    Arc::new(PatchTool { coerces: false })
}

#[test]
fn test_validate_freeform_raw_string_coerces_to_typed_object() {
    let tool = freeform_tool();
    let raw = "*** Begin Patch\n*** End Patch\n";
    let validated = ValidatedInput::validate(tool.as_ref(), Value::String(raw.to_string()))
        .expect("freeform raw string must coerce and validate");
    assert_eq!(validated.as_value(), &json!({ "patch": raw }));
    // The coerced shape must round-trip into the tool's typed input —
    // exactly what `DynTool::execute` does at execution time.
    let typed: PatchInput =
        serde_json::from_value(validated.into_value()).expect("typed deserialization");
    assert_eq!(typed.patch, raw);
}

#[test]
fn test_validate_object_input_passes_through_unchanged() {
    let tool = freeform_tool();
    let input = json!({ "patch": "body" });
    let validated = ValidatedInput::validate(tool.as_ref(), input.clone()).expect("valid object");
    assert_eq!(validated.into_value(), input);
}

#[test]
fn test_validate_raw_string_on_function_tool_fails_schema() {
    let tool = function_tool();
    let result = ValidatedInput::validate(tool.as_ref(), Value::String("not json".into()));
    assert!(
        result.is_err(),
        "non-coercing tool must reject a bare string via schema validation"
    );
}

#[test]
fn test_validate_schema_violation_reports_issues() {
    let tool = freeform_tool();
    let result = ValidatedInput::validate(tool.as_ref(), json!({ "wrong_field": 1 }));
    assert!(result.is_err(), "missing required field must fail");
}
