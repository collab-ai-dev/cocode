//! Test fixtures and data generators for integration tests.
//!
//! This module provides reusable test data including prompts, tools,
//! and result extractors for testing various provider features.

#![allow(dead_code)]

use std::sync::Arc;

use serde_json::json;
use vercel_ai::AISdkError;
use vercel_ai::GenerateTextResult;
use vercel_ai::JSONValue;
use vercel_ai::ToolRegistry;
use vercel_ai::dynamic_tool;

/// Create a weather tool for testing.
pub fn weather_tool() -> vercel_ai::SimpleTool {
    dynamic_tool(
        "get_weather",
        "Get the current weather for a city",
        json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "The city name"
                }
            },
            "required": ["city"]
        }),
        |_input: JSONValue, _options| async move {
            Ok(json!({
                "temperature": "22C",
                "condition": "sunny",
                "humidity": "45%"
            }))
        },
    )
}

/// Create a weather tool registry for testing.
pub fn weather_tool_registry() -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(weather_tool()));
    Arc::new(registry)
}

/// Create a non-executable weather tool (returns error, for testing tool call detection only).
pub fn weather_tool_no_exec() -> vercel_ai::SimpleTool {
    dynamic_tool(
        "get_weather",
        "Get the current weather for a city",
        json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "The city name"
                }
            },
            "required": ["city"]
        }),
        |_input: JSONValue, _options| async move {
            Err(AISdkError::new("Tool execution not supported in this test"))
        },
    )
}

/// Create a non-executable weather tool registry.
pub fn weather_tool_registry_no_exec() -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(weather_tool_no_exec()));
    Arc::new(registry)
}

/// Check if result contains a tool call with the given name.
pub fn has_tool_call_named(result: &GenerateTextResult, name: &str) -> bool {
    result.tool_calls.iter().any(|tc| tc.tool_name == name)
}

/// A small 10x10 red square PNG image encoded as base64 data URL.
pub const TEST_RED_SQUARE_BASE64: &str = "data:image/png;base64,\
iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAYAAACNMs+9AAAAFUlEQVR4AWNgGAWjYBSMglEwCkgHAA+IAAT6kbF5AAAAAElFTkSuQmCC";

/// A small 10x10 blue square PNG image encoded as base64 data URL.
pub const TEST_BLUE_SQUARE_BASE64: &str = "data:image/png;base64,\
iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAYAAACNMs+9AAAAFUlEQVR4AWP4//8/w0AmGAWjgHIAABZQAQVmGY6GAAAAAElFTkSuQmCC";

/// A real-world JPEG image (1280x1280) with gold Chinese text "2025" on dark background.
/// Used for vision tests that need a realistic image (e.g., Google Gemini lite models
/// reject tiny synthetic PNGs).
pub const TEST_VISION_IMAGE_BYTES: &[u8] = include_bytes!("../../share.jpg");
