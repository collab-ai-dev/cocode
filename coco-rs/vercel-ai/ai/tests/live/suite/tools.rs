//! Tool calling tests.
//!
//! Tests tool/function calling capabilities including single-turn and complete flow.

use std::sync::Arc;

use anyhow::Result;
use vercel_ai::GenerateTextOptions;
use vercel_ai::LanguageModel;
use vercel_ai::LanguageModelV4;
use vercel_ai::Prompt;
use vercel_ai::generate_text;

use crate::common::has_tool_call_named;
use crate::common::weather_tool_registry;
use crate::common::weather_tool_registry_no_exec;

/// Test basic tool calling.
///
/// Verifies that the model can generate function calls.
pub async fn run(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let tools = weather_tool_registry_no_exec();

    let result = generate_text(
        GenerateTextOptions::new(
            LanguageModel::from_v4(model.clone()),
            Prompt::user("What's the weather in Tokyo? Use the get_weather tool.").with_system(
                "You are a helpful assistant. Use the provided tools when appropriate.",
            ),
        )
        .with_tools(tools),
    )
    .await?;

    assert!(
        has_tool_call_named(&result, "get_weather"),
        "Expected get_weather function call in response"
    );
    Ok(())
}

/// Test complete tool calling flow.
///
/// Verifies the full workflow: question -> tool call -> tool execution -> final response.
/// Uses `max_steps: 2` so the model can call the tool and then produce a final answer.
pub async fn run_complete_flow(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let tools = weather_tool_registry();

    let result = generate_text(
        GenerateTextOptions::new(
            LanguageModel::from_v4(model.clone()),
            Prompt::user("What's the weather in Tokyo?").with_system(
                "You are a helpful assistant. Use the provided tools when appropriate.",
            ),
        )
        .with_tools(tools)
        .with_max_steps(2),
    )
    .await?;

    // With max_steps=2, the model should have called the tool and then produced a response
    assert!(
        result.text.to_lowercase().contains("22")
            || result.text.to_lowercase().contains("sunny")
            || result.text.to_lowercase().contains("weather"),
        "Expected response to include weather info from tool output, got: {}",
        result.text
    );
    Ok(())
}
