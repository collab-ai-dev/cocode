//! Streaming tests.
//!
//! Tests streaming generation capabilities.

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use vercel_ai::LanguageModel;
use vercel_ai::LanguageModelV4;
use vercel_ai::Prompt;
use vercel_ai::StreamTextOptions;
use vercel_ai::TextStreamPart;
use vercel_ai::stream_text;

use crate::common::weather_tool_registry_no_exec;

/// Test basic streaming generation.
///
/// Verifies that the model can stream text responses.
pub async fn run(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let result = stream_text(StreamTextOptions::new(
        LanguageModel::from_v4(model.clone()),
        Prompt::user("Say 'hello world' exactly.")
            .with_system("You are a helpful assistant. Be concise."),
    ));

    let mut stream = result.stream;
    let mut collected_text = String::new();
    let mut event_count = 0;
    let mut has_finish = false;

    while let Some(part) = stream.next().await {
        event_count += 1;
        match part {
            TextStreamPart::TextDelta { delta, .. } => {
                collected_text.push_str(&delta);
            }
            TextStreamPart::Finish { .. } => {
                has_finish = true;
            }
            _ => {}
        }
    }

    assert!(event_count > 0, "Expected at least one stream event");
    assert!(has_finish, "Expected Finish event");
    assert!(
        collected_text.to_lowercase().contains("hello"),
        "Expected 'hello' in streamed text, got: {collected_text}"
    );

    Ok(())
}

/// Test streaming with tool calls.
///
/// Verifies that tool calls are properly streamed.
pub async fn run_with_tools(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let tools = weather_tool_registry_no_exec();

    let result = stream_text(
        StreamTextOptions::new(
            LanguageModel::from_v4(model.clone()),
            Prompt::user("What's the weather in Tokyo? Use the get_weather tool.").with_system(
                "You are a helpful assistant. Use the provided tools when appropriate.",
            ),
        )
        .with_tools(tools),
    );

    let mut stream = result.stream;
    let mut has_tool_call_start = false;
    let mut tool_name = String::new();

    while let Some(part) = stream.next().await {
        if let TextStreamPart::ToolCallStart {
            tool_name: name, ..
        } = part
        {
            has_tool_call_start = true;
            tool_name = name;
        }
    }

    assert!(
        has_tool_call_start,
        "Expected ToolCallStart event for weather tool"
    );
    assert_eq!(
        tool_name, "get_weather",
        "Expected get_weather tool call, got: {tool_name}"
    );

    Ok(())
}
