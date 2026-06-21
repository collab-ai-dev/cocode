//! Tests for tool execution types.

use super::*;
use async_trait::async_trait;
use serde_json::json;

struct TestTool {
    definition: LanguageModelV4FunctionTool,
}

impl TestTool {
    fn new(name: &str) -> Self {
        Self {
            definition: LanguageModelV4FunctionTool::with_description(
                name,
                format!("Test tool {name}"),
                json!({ "type": "object" }),
            ),
        }
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
async fn test_tool_execution_options() {
    let options = ToolExecutionOptions::new("call_123");
    assert_eq!(options.tool_call_id, "call_123");
    assert!(options.messages.is_empty());
    assert!(options.abort_signal.is_none());
    assert!(options.experimental_context.is_none());
}

#[tokio::test]
async fn test_tool_execution_options_with_messages() {
    let options = ToolExecutionOptions::new("call_123").with_messages(vec![]);
    assert_eq!(options.tool_call_id, "call_123");
    assert!(options.messages.is_empty());
}

#[tokio::test]
async fn test_simple_tool() {
    let tool = SimpleTool::new(
        LanguageModelV4FunctionTool::with_description(
            "add",
            "Add two numbers",
            json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number" },
                    "b": { "type": "number" }
                }
            }),
        ),
        |input, _options| async move {
            let a = input
                .get("a")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let b = input
                .get("b")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            Ok(json!({ "result": a + b }))
        },
    );

    let options = ToolExecutionOptions::new("call_test");
    let result = tool.execute(json!({ "a": 5, "b": 3 }), options).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!({ "result": 8 }));
}

#[tokio::test]
async fn test_simple_tool_builder() {
    let tool = SimpleTool::with_name("multiply")
        .description("Multiply two numbers")
        .parameters(json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": { "type": "number" }
            }
        }))
        .handler(|input, _options| async move {
            let a = input
                .get("a")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let b = input
                .get("b")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            Ok(json!({ "result": a * b }))
        });

    assert_eq!(tool.definition.name, "multiply");

    let options = ToolExecutionOptions::new("call_builder");
    let result = tool.execute(json!({ "a": 4, "b": 7 }), options).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!({ "result": 28 }));
}

#[tokio::test]
async fn test_tool_registry() {
    let mut registry = ToolRegistry::new();

    let tool = Arc::new(TestTool::new("test1"));
    registry.register(tool);

    let tool = Arc::new(TestTool::new("test2"));
    registry.register(tool);

    assert!(registry.get("test1").is_some());
    assert!(registry.get("test2").is_some());
    assert!(registry.get("unknown").is_none());

    let definitions = registry.definitions();
    assert_eq!(definitions.len(), 2);
}

#[tokio::test]
async fn test_tool_registry_execute() {
    let mut registry = ToolRegistry::new();

    let tool = Arc::new(SimpleTool::new(
        LanguageModelV4FunctionTool::with_description(
            "double",
            "Double a number",
            json!({ "type": "object", "properties": { "n": { "type": "number" } } }),
        ),
        |input, _options| async move {
            let n = input
                .get("n")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            Ok(json!({ "result": n * 2 }))
        },
    ));
    registry.register(tool);

    let options = ToolExecutionOptions::new("call_123");
    let result = registry.execute("double", json!({ "n": 5 }), options).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!({ "result": 10 }));

    let options = ToolExecutionOptions::new("call_456");
    let result = registry.execute("unknown", json!({}), options).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_tool_execution_with_options() {
    let tool = SimpleTool::new(
        LanguageModelV4FunctionTool::with_description(
            "echo_id",
            "Echo the tool call ID",
            json!({ "type": "object" }),
        ),
        |_input, options| async move { Ok(json!({ "tool_call_id": options.tool_call_id })) },
    );

    let options = ToolExecutionOptions::new("my_call_id");
    let result = tool.execute(json!({}), options).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!({ "tool_call_id": "my_call_id" }));
}
