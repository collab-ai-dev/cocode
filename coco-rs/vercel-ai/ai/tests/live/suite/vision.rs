//! Vision/image understanding tests.
//!
//! Tests multi-modal image understanding capabilities via the vercel-ai
//! unified message format.

use std::sync::Arc;

use anyhow::Result;
use vercel_ai::GenerateTextOptions;
use vercel_ai::LanguageModel;
use vercel_ai::LanguageModelV4;
use vercel_ai::Prompt;
use vercel_ai::UserContentPart;
use vercel_ai::generate_text;
use vercel_ai_provider::LanguageModelV4Message;

use crate::common::TEST_VISION_IMAGE_BYTES;

/// Test image understanding.
///
/// Sends a real-world JPEG image containing "2025" text and asks the model
/// to identify the year shown.
pub async fn run(model: &Arc<dyn LanguageModelV4>) -> Result<()> {
    let result = generate_text(GenerateTextOptions::new(
        LanguageModel::from_v4(model.clone()),
        Prompt::messages(vec![LanguageModelV4Message::user(vec![
            UserContentPart::image(TEST_VISION_IMAGE_BYTES.to_vec(), "image/jpeg"),
            UserContentPart::text("What year is shown in this image? Answer with just the number."),
        ])])
        .with_system("You are a helpful assistant. Be concise."),
    ))
    .await?;

    assert!(
        !result.text.is_empty(),
        "Expected non-empty response for vision input"
    );

    Ok(())
}
