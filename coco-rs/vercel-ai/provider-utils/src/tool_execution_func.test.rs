//! Tests for tool execution functions.

use super::*;
use crate::types::ExecutableTool;
use crate::types::ToolExecutionOptions;
use async_trait::async_trait;
use serde_json::json;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

struct TestTool {
    definition: LanguageModelV4FunctionTool,
}

impl TestTool {
    fn new(name: &str) -> Self {
        let mut def = LanguageModelV4FunctionTool::new(name, json!({ "type": "object" }));
        def.description = Some(format!("Test tool {name}"));
        Self { definition: def }
    }
}

#[async_trait]
impl ExecutableTool for TestTool {
    fn definition(&self) -> &LanguageModelV4FunctionTool {
        &self.definition
    }

    async fn execute(
        &self,
        input: JSONValue,
        _options: ToolExecutionOptions,
    ) -> Result<JSONValue, AISdkError> {
        Ok(input)
    }
}

#[tokio::test]
async fn test_execute_tool() {
    let tool = TestTool::new("test");
    let input = json!({ "value": 42 });
    let options = ToolExecutionOptions::new("call_123");
    let result = execute_tool(&tool, input.clone(), options).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), input);
}

#[tokio::test]
async fn test_dynamic_tool() {
    let tool = dynamic_tool(
        "echo",
        "Echo the input",
        json!({ "type": "object", "properties": { "message": { "type": "string" } } }),
        |input, _options| async move { Ok(input) },
    );

    let input = json!({ "message": "hello" });
    let options = ToolExecutionOptions::new("call_dynamic");
    let result = tool.execute(input.clone(), options).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), input);
}

#[tokio::test]
async fn test_dynamic_tool_with_options() {
    let tool = dynamic_tool(
        "echo_id",
        "Echo the tool call ID",
        json!({ "type": "object" }),
        |_input, options| async move { Ok(json!({ "tool_call_id": options.tool_call_id })) },
    );

    let options = ToolExecutionOptions::new("my_special_call_id");
    let result = tool.execute(json!({}), options).await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        json!({ "tool_call_id": "my_special_call_id" })
    );
}
