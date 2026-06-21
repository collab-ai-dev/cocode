//! Tool execution functions.
//!
//! This module provides utility functions for executing tools and creating dynamic tools.

use serde_json::Value as JSONValue;

use vercel_ai_provider::AISdkError;

use crate::types::ExecutableTool;
use crate::types::SimpleTool;
use crate::types::ToolExecutionOptions;

/// Execute a tool with the given input and options.
///
/// This function validates the input against the tool's schema (if present),
/// executes the tool, and returns the result.
///
/// # Arguments
///
/// * `tool` - The tool to execute
/// * `input` - The input to pass to the tool
/// * `options` - The execution options (tool call ID, messages, abort signal, etc.)
///
/// # Example
///
/// ```ignore
/// use vercel_ai_provider_utils::{SimpleTool, execute_tool, ToolExecutionOptions};
/// use vercel_ai_provider::ToolDefinitionV4;
/// use serde_json::json;
///
/// let tool = SimpleTool::new(
///     ToolDefinitionV4::function("echo", "Echoes the input", json!({ "type": "object" })),
///     |input, _options| async move { Ok(input) }
/// );
///
/// let options = ToolExecutionOptions::new("call_123");
/// let result = execute_tool(&tool, json!({"message": "hello"}), options).await;
/// ```
pub async fn execute_tool(
    tool: &dyn ExecutableTool,
    input: JSONValue,
    options: ToolExecutionOptions,
) -> Result<JSONValue, AISdkError> {
    tool.execute(input, options).await
}

/// Create a dynamic tool.
///
/// A dynamic tool is one where the input schema is determined at runtime
/// rather than compile time. This is useful for tools that have
/// variable input structures.
///
/// # Arguments
///
/// * `name` - The tool name
/// * `description` - The tool description
/// * `parameters` - The JSON schema for the input parameters
/// * `handler` - The function to execute when the tool is called
///
/// # Example
///
/// ```ignore
/// use vercel_ai_provider_utils::dynamic_tool;
/// use serde_json::json;
///
/// let tool = dynamic_tool(
///     "search",
///     "Search for documents",
///     json!({
///         "type": "object",
///         "properties": {
///             "query": { "type": "string" }
///         }
///     }),
///     |input, _options| async move {
///         let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
///         Ok(json!({ "results": [] }))
///     }
/// );
/// ```
pub fn dynamic_tool<F, Fut>(
    name: impl Into<String>,
    description: impl Into<String>,
    parameters: JSONValue,
    handler: F,
) -> SimpleTool
where
    F: Fn(JSONValue, ToolExecutionOptions) -> Fut + Send + Sync + 'static,
    Fut: futures::Future<Output = Result<JSONValue, AISdkError>> + Send + 'static,
{
    let mut tool_def =
        vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool::new(
            name, parameters,
        );
    tool_def.description = Some(description.into());
    SimpleTool::new(tool_def, handler)
}

#[cfg(test)]
#[path = "tool_execution_func.test.rs"]
mod tests;
