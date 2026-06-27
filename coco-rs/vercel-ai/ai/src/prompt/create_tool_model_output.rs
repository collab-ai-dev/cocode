//! Create tool model output.
//!
//! This module provides functionality for creating tool model outputs
//! from tool execution results.

use serde_json::Value as JSONValue;
use serde_json::json;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;

/// Create a tool result content from a JSON value.
///
/// # Arguments
///
/// * `value` - The JSON value to convert.
///
/// # Returns
///
/// A `ToolResultContent` suitable for sending to the model.
pub fn create_tool_result_content(value: &JSONValue) -> ToolResultContent {
    // Convert JSON to string representation
    match value {
        JSONValue::String(s) => ToolResultContent::text(s),
        JSONValue::Array(arr) => {
            // For arrays, convert to newline-separated text
            let text = arr
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            ToolResultContent::text(text)
        }
        JSONValue::Object(_) => {
            // For objects, serialize to JSON string
            ToolResultContent::text(serde_json::to_string(value).unwrap_or_default())
        }
        _ => ToolResultContent::text(value.to_string()),
    }
}

/// Create a tool result part from tool execution.
///
/// # Arguments
///
/// * `tool_call_id` - The tool call ID.
/// * `tool_name` - The tool name.
/// * `result` - The tool result.
/// * `is_error` - Whether the result is an error.
///
/// # Returns
///
/// A `ToolResultPart` suitable for sending to the model.
pub fn create_tool_result_part(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    result: JSONValue,
    is_error: bool,
) -> ToolResultPart {
    let content = if is_error {
        ToolResultContent::text(format!("Error: {result}"))
    } else {
        create_tool_result_content(&result)
    };

    ToolResultPart::new(tool_call_id.into(), tool_name.into(), content)
}

/// Create a tool result part from text.
///
/// # Arguments
///
/// * `tool_call_id` - The tool call ID.
/// * `tool_name` - The tool name.
/// * `text` - The text result.
///
/// # Returns
///
/// A `ToolResultPart` with text content.
pub fn create_tool_result_part_from_text(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    text: impl Into<String>,
) -> ToolResultPart {
    ToolResultPart::new(
        tool_call_id.into(),
        tool_name.into(),
        ToolResultContent::text(text),
    )
}

/// Create a tool result part from an error.
///
/// # Arguments
///
/// * `tool_call_id` - The tool call ID.
/// * `tool_name` - The tool name.
/// * `error` - The error message.
///
/// # Returns
///
/// A `ToolResultPart` with error content.
pub fn create_tool_result_part_from_error(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    error: impl Into<String>,
) -> ToolResultPart {
    ToolResultPart::new(
        tool_call_id.into(),
        tool_name.into(),
        ToolResultContent::text(format!("Error: {}", error.into())),
    )
}

/// Create tool model output for image content.
///
/// # Arguments
///
/// * `tool_call_id` - The tool call ID.
/// * `tool_name` - The tool name.
/// * `image_data` - Base64-encoded image data.
/// * `media_type` - The image media type (e.g., "image/png").
///
/// # Returns
///
/// A `ToolResultPart` with image content.
pub fn create_tool_result_part_from_image(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    image_data: impl Into<String>,
    media_type: impl Into<String>,
) -> ToolResultPart {
    // Create JSON content with image data
    let image_json = json!({
        "type": "image",
        "data": image_data.into(),
        "mediaType": media_type.into()
    });
    let content = ToolResultContent::json(image_json);
    ToolResultPart::new(tool_call_id.into(), tool_name.into(), content)
}
