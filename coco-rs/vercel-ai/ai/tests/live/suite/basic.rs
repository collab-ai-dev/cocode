//! Basic text generation tests.
//!
//! Tests simple text completion, token usage reporting, and multi-turn conversations.

use std::sync::Arc;

use anyhow::Result;
use vercel_ai::GenerateTextOptions;
use vercel_ai::LanguageModel;
use vercel_ai::LanguageModelV4;
use vercel_ai::Prompt;
use vercel_ai::generate_text;
use vercel_ai_provider::LanguageModelV4Message;

/// Test basic text generation.
///
/// Verifies that the model can generate simple text responses.
pub async fn run(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model.clone()),
        Prompt::user("Say 'hello' in exactly one word, nothing else.")
            .with_system("You are a helpful assistant. Be concise."),
    ))
    .await?;

    assert!(
        result.text.to_lowercase().contains("hello"),
        "Expected 'hello' in response, got: {}",
        result.text
    );
    Ok(())
}

/// Test token usage reporting.
///
/// Verifies that the model reports token usage statistics.
pub async fn run_token_usage(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model.clone()),
        Prompt::user("Say 'hello'.").with_system("You are a helpful assistant. Be concise."),
    ))
    .await?;

    assert!(
        result.usage.input_tokens.total().unwrap_or(0) > 0,
        "Expected non-zero input tokens"
    );
    assert!(
        result.usage.output_tokens.total.unwrap_or(0) > 0,
        "Expected non-zero output tokens"
    );
    Ok(())
}

/// Test multi-turn conversation.
///
/// Verifies that the model preserves context across conversation turns.
pub async fn run_multi_turn(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model.clone()),
        Prompt::messages(vec![
            LanguageModelV4Message::system("You are a helpful assistant."),
            LanguageModelV4Message::user_text("My name is TestUser. Please remember it."),
            LanguageModelV4Message::assistant_text("Hello TestUser! I'll remember your name."),
            LanguageModelV4Message::user_text("What is my name?"),
        ]),
    ))
    .await?;

    assert!(
        result.text.to_lowercase().contains("testuser"),
        "Expected 'testuser' in response (context should be preserved), got: {}",
        result.text
    );
    Ok(())
}
