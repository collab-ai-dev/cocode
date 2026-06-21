//! Execute tool calls.
//!
//! This module provides functionality for executing individual tool calls
//! and handling the results.

use std::sync::Arc;

use serde_json::Value as JSONValue;

use crate::error::NoSuchToolError;
use crate::types::ToolRegistry;

use super::generate_text_result::ToolCall;
use super::tool_error::ToolError;
use super::tool_output::ToolOutput;

/// Options for tool execution.
pub use vercel_ai_provider_utils::ToolExecutionOptions;

/// Execute a single tool call.
///
/// # Arguments
///
/// * `tool_call` - The tool call to execute.
/// * `tools` - The tool registry.
/// * `options` - Execution options.
///
/// # Returns
///
/// The tool output or an error.
pub async fn execute_tool_call(
    tool_call: &ToolCall,
    tools: &Arc<ToolRegistry>,
    options: ToolExecutionOptions,
) -> Result<JSONValue, ToolError> {
    // Check if tool exists
    if tools.get(&tool_call.tool_name).is_none() {
        return Err(ToolError::new(
            &tool_call.tool_call_id,
            &tool_call.tool_name,
            serde_json::Value::String(format!("Tool '{}' not found", tool_call.tool_name)),
        ));
    }

    // Execute the tool
    let result = tools
        .execute(&tool_call.tool_name, tool_call.args.clone(), options)
        .await;

    match result {
        Ok(output) => Ok(output),
        Err(e) => Err(ToolError::new(
            &tool_call.tool_call_id,
            &tool_call.tool_name,
            serde_json::Value::String(e.to_string()),
        )),
    }
}

/// Execute multiple tool calls in parallel.
///
/// # Arguments
///
/// * `tool_calls` - The tool calls to execute.
/// * `tools` - The tool registry.
/// * `options_fn` - A function to create options for each tool call.
///
/// # Returns
///
/// A vector of tool outputs (successful) and errors.
pub async fn execute_tool_calls<F>(
    tool_calls: &[ToolCall],
    tools: &Arc<ToolRegistry>,
    options_fn: F,
) -> Vec<Result<JSONValue, ToolError>>
where
    F: Fn(&ToolCall) -> ToolExecutionOptions + Send + Sync,
{
    let mut results = Vec::with_capacity(tool_calls.len());

    for tc in tool_calls {
        let options = options_fn(tc);
        let result = execute_tool_call(tc, tools, options).await;
        results.push(result);
    }

    results
}

/// Execute tool calls with concurrency limit.
///
/// # Arguments
///
/// * `tool_calls` - The tool calls to execute.
/// * `tools` - The tool registry.
/// * `options_fn` - A function to create options for each tool call.
/// * `concurrency` - Maximum number of concurrent executions.
///
/// # Returns
///
/// A vector of tool outputs (successful) and errors.
pub async fn execute_tool_calls_with_concurrency<F>(
    tool_calls: &[ToolCall],
    tools: &Arc<ToolRegistry>,
    options_fn: F,
    concurrency: usize,
) -> Vec<Result<JSONValue, ToolError>>
where
    F: Fn(&ToolCall) -> ToolExecutionOptions + Send + Sync + Clone,
{
    use futures::stream::StreamExt;
    use futures::stream::{self};

    let tools = tools.clone();
    let options_fn = options_fn.clone();

    let results: Vec<Result<JSONValue, ToolError>> = stream::iter(tool_calls)
        .map(move |tc| {
            let tools = tools.clone();
            let options_fn = options_fn.clone();
            async move {
                let options = options_fn(tc);
                execute_tool_call(tc, &tools, options).await
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    // Sort results by tool call order
    let mut indexed_results: Vec<_> = results.into_iter().enumerate().collect();
    indexed_results.sort_by_key(|(i, _)| *i);
    indexed_results.into_iter().map(|(_, r)| r).collect()
}

/// Check if a tool call is valid.
///
/// # Arguments
///
/// * `tool_call` - The tool call to check.
/// * `tools` - The tool registry.
///
/// # Returns
///
/// Ok if valid, Err with NoSuchToolError if tool doesn't exist.
pub fn validate_tool_call(
    tool_call: &ToolCall,
    tools: &Arc<ToolRegistry>,
) -> Result<(), NoSuchToolError> {
    if tools.get(&tool_call.tool_name).is_none() {
        return Err(NoSuchToolError::new(&tool_call.tool_name));
    }
    Ok(())
}

/// Validate all tool calls.
///
/// # Arguments
///
/// * `tool_calls` - The tool calls to validate.
/// * `tools` - The tool registry.
///
/// # Returns
///
/// Ok if all valid, Err with the first invalid tool.
pub fn validate_tool_calls(
    tool_calls: &[ToolCall],
    tools: &Arc<ToolRegistry>,
) -> Result<(), NoSuchToolError> {
    for tc in tool_calls {
        validate_tool_call(tc, tools)?;
    }
    Ok(())
}

/// Convert tool output to tool result content.
pub fn output_to_result_content(output: &ToolOutput) -> vercel_ai_provider::ToolResultContent {
    match output {
        ToolOutput::Json(value) => vercel_ai_provider::ToolResultContent::text(
            serde_json::to_string(value).unwrap_or_default(),
        ),
        ToolOutput::Text(text) => vercel_ai_provider::ToolResultContent::text(text),
        ToolOutput::Multi(parts) => {
            // Convert multi-part to text
            let text = parts
                .iter()
                .map(|p| match p {
                    super::tool_output::ToolOutputContent::Text { text } => text.clone(),
                    super::tool_output::ToolOutputContent::Image { image, .. } => image.clone(),
                    super::tool_output::ToolOutputContent::Json { value } => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            vercel_ai_provider::ToolResultContent::text(text)
        }
    }
}
