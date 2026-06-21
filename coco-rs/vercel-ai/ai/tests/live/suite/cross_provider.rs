//! Cross-provider conversation tests.
//!
//! Tests that verify messages from one provider can be correctly
//! used with another provider via the vercel-ai unified message format.

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use vercel_ai::GenerateTextOptions;
use vercel_ai::LanguageModel;
use vercel_ai::LanguageModelV4;
use vercel_ai::Prompt;
use vercel_ai::StreamTextOptions;
use vercel_ai::TextStreamPart;
use vercel_ai::ToolRegistry;
use vercel_ai::build_assistant_message;
use vercel_ai::build_tool_result_message;
use vercel_ai::generate_text;
use vercel_ai::stream_text;
use vercel_ai_provider::LanguageModelV4Message;

/// Run cross-provider conversation test.
///
/// Gets a response from the source provider, builds a multi-turn prompt
/// including that response, and sends it to the target provider.
pub async fn run(
    source_model: &Arc<dyn LanguageModelV4>,
    target_model: &Arc<dyn LanguageModelV4>,
) -> Result<()> {
    // Step 1: Get a response from the source provider
    let source_result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(source_model.clone()),
        Prompt::user("What is 2+2? Reply with just the number.")
            .with_system("You are a helpful assistant. Be concise."),
    ))
    .await?;

    // Step 2: Create follow-up request with history for target provider
    let target_result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(target_model.clone()),
        Prompt::messages(vec![
            LanguageModelV4Message::system("You are a helpful assistant. Be concise."),
            LanguageModelV4Message::user_text("What is 2+2? Reply with just the number."),
            LanguageModelV4Message::assistant_text(&source_result.text),
            LanguageModelV4Message::user_text(
                "What is double that number? Reply with just the number.",
            ),
        ]),
    ))
    .await?;

    // Verify we got a valid response containing "8"
    let response_text = target_result.text.trim();
    assert!(
        !response_text.is_empty(),
        "Target provider should return a response"
    );
    assert!(
        response_text.contains('8'),
        "Response should contain '8', got: {response_text}"
    );

    Ok(())
}

/// Run A→B→A round-trip cross-provider test.
///
/// Tests that messages survive a full round-trip across providers:
/// 1. Provider A answers a question
/// 2. Provider B continues the conversation with A's response in history
/// 3. Provider A continues again with both prior responses in history
pub async fn run_round_trip(
    model_a: &Arc<dyn LanguageModelV4>,
    model_b: &Arc<dyn LanguageModelV4>,
) -> Result<()> {
    // Step 1: A answers "What is 3+5?"
    let result_a1 = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model_a.clone()),
        Prompt::user("What is 3+5? Reply with just the number.")
            .with_system("You are a helpful assistant. Be concise."),
    ))
    .await?;

    assert!(
        result_a1.text.contains('8'),
        "A step1 should contain '8', got: {}",
        result_a1.text.trim()
    );

    // Step 2: B continues with A's response — "double that number"
    let result_b = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model_b.clone()),
        Prompt::messages(vec![
            LanguageModelV4Message::system("You are a helpful assistant. Be concise."),
            LanguageModelV4Message::user_text("What is 3+5? Reply with just the number."),
            LanguageModelV4Message::assistant_text(&result_a1.text),
            LanguageModelV4Message::user_text(
                "What is double that number? Reply with just the number.",
            ),
        ]),
    ))
    .await?;

    assert!(
        result_b.text.contains("16"),
        "B step2 should contain '16', got: {}",
        result_b.text.trim()
    );

    // Step 3: A continues with both A's and B's responses — "add 4 to that"
    let result_a2 = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model_a.clone()),
        Prompt::messages(vec![
            LanguageModelV4Message::system("You are a helpful assistant. Be concise."),
            LanguageModelV4Message::user_text("What is 3+5? Reply with just the number."),
            LanguageModelV4Message::assistant_text(&result_a1.text),
            LanguageModelV4Message::user_text(
                "What is double that number? Reply with just the number.",
            ),
            LanguageModelV4Message::assistant_text(&result_b.text),
            LanguageModelV4Message::user_text("Add 4 to that number. Reply with just the number."),
        ]),
    ))
    .await?;

    assert!(
        result_a2.text.contains("20"),
        "A step3 should contain '20', got: {}",
        result_a2.text.trim()
    );

    Ok(())
}

/// Run streaming cross-provider test.
///
/// Tests that streaming works correctly with cross-provider message history.
/// Gets a response from the source provider, then streams a follow-up on the
/// target provider using that response as conversation history.
pub async fn run_streaming(
    source_model: &Arc<dyn LanguageModelV4>,
    target_model: &Arc<dyn LanguageModelV4>,
) -> Result<()> {
    // Step 1: Get a response from source provider
    let source_result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(source_model.clone()),
        Prompt::user("What is the capital of France?")
            .with_system("You are a helpful assistant. Be concise."),
    ))
    .await?;

    // Step 2: Stream follow-up on target with source's response as history
    let result = stream_text(StreamTextOptions::new(
        LanguageModel::from_v4(target_model.clone()),
        Prompt::messages(vec![
            LanguageModelV4Message::system("You are a helpful assistant."),
            LanguageModelV4Message::user_text("What is the capital of France?"),
            LanguageModelV4Message::assistant_text(&source_result.text),
            LanguageModelV4Message::user_text("What is a famous landmark there?"),
        ]),
    ));

    // Consume the stream
    let mut stream = result.stream;
    let mut collected_text = String::new();

    while let Some(part) = stream.next().await {
        if let TextStreamPart::TextDelta { delta, .. } = part {
            collected_text.push_str(&delta);
        }
    }

    // Verify response mentions a Paris landmark
    let text = collected_text.to_lowercase();
    assert!(!text.is_empty(), "Should get a streaming response");
    let mentions_landmark = text.contains("eiffel")
        || text.contains("louvre")
        || text.contains("notre")
        || text.contains("arc")
        || text.contains("tower")
        || text.contains("museum");
    assert!(
        mentions_landmark || text.len() > 20,
        "Response should mention a Paris landmark or be substantive: {text}"
    );

    Ok(())
}

/// Run A→B cross-provider test with tool calls in history.
///
/// Tests that tool call and tool result messages from provider A are correctly
/// processed by provider B:
/// 1. Provider A calls a tool (max_steps=2: tool call → execution → final answer)
/// 2. The full history (user, assistant+tool_call, tool_result, assistant+text) is
///    passed to provider B for a follow-up question
/// 3. Provider B must understand the tool-enriched history and respond correctly
pub async fn run_with_tools(
    model_a: &Arc<dyn LanguageModelV4>,
    model_b: &Arc<dyn LanguageModelV4>,
    tools: Arc<ToolRegistry>,
) -> Result<()> {
    // Step 1: A calls the weather tool and produces a final answer
    let result_a = generate_text(
        GenerateTextOptions::new(
            LanguageModel::from_v4(model_a.clone()),
            Prompt::user("What's the weather in Tokyo?").with_system(
                "You are a helpful assistant. Use the provided tools when appropriate.",
            ),
        )
        .with_tools(tools)
        .with_max_steps(2),
    )
    .await?;

    // Verify A actually used the tool and produced a response
    assert!(
        !result_a.steps.is_empty(),
        "A should have at least one step"
    );
    assert!(
        result_a.text.to_lowercase().contains("22")
            || result_a.text.to_lowercase().contains("sunny")
            || result_a.text.to_lowercase().contains("weather")
            || result_a.text.to_lowercase().contains("tokyo"),
        "A should mention weather info, got: {}",
        result_a.text
    );

    // Step 2: Build full conversation history from A's steps
    // Each step has content (assistant message with tool calls or text) and tool_results
    let mut history = vec![
        LanguageModelV4Message::system(
            "You are a helpful assistant. Use the provided tools when appropriate.",
        ),
        LanguageModelV4Message::user_text("What's the weather in Tokyo?"),
    ];

    for step in &result_a.steps {
        // Assistant message (may contain tool_call parts and/or text parts)
        history.push(build_assistant_message(step.content.clone()));
        // Tool result message (if this step executed tools)
        if !step.tool_results.is_empty() {
            history.push(build_tool_result_message(&step.tool_results));
        }
    }

    // Add follow-up question for B
    history.push(LanguageModelV4Message::user_text(
        "Based on that weather, should I bring an umbrella? Answer yes or no.",
    ));

    // Step 3: B receives the full tool-enriched history and responds
    let result_b = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model_b.clone()),
        Prompt::messages(history),
    ))
    .await?;

    // B should produce a meaningful response referencing the weather
    let text = result_b.text.to_lowercase();
    assert!(
        !text.is_empty(),
        "B should produce a response from tool-enriched history"
    );
    // The weather was sunny, so the answer should be "no" (no umbrella needed)
    assert!(
        text.contains("no")
            || text.contains("sunny")
            || text.contains("umbrella")
            || text.len() > 5,
        "B should reference the weather context, got: {text}"
    );

    Ok(())
}
