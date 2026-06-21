//! Multimodal (image-input) tests via `coco_inference::ModelRuntimeClient::query`.
//!
//! Mirrors the OpenAI Responses curl example:
//!
//! ```text
//! POST <base_url>/responses
//! { "model": "...", "input": [
//!     { "role": "user", "content": [
//!         { "type": "input_text",  "text": "what is in this image?" },
//!         { "type": "input_image", "image_url": "https://..." }
//!     ] } ] }
//! ```
//!
//! The `UserContentPart::image_url` part is converted to the
//! `{ "type": "input_image", "image_url": "<url>" }` shape by
//! `vercel-ai-openai/src/responses/convert_to_responses_input.rs`.

use anyhow::Result;
use coco_inference::QueryParams;
use coco_llm_types::LlmMessage;
use coco_llm_types::UserContentPart;

use crate::common::LiveTarget;
use crate::common::extract_text;
use crate::common::query_client;
use crate::common::usage_report;

const SYSTEM: &str = "You are a helpful assistant. Be concise.";

/// 1280x1280 JPEG containing the text "2025". Inlined so the test
/// never depends on an outbound URL fetch from the provider gateway —
/// some gateways (e.g. AIDP modelhub) try to download remote
/// `image_url` values server-side and reject hosts they can't reach.
/// The SDK rewraps these bytes as a `data:image/jpeg;base64,…` URL
/// when emitting the Responses `input_image` part. Shared with the
/// `vercel-ai/ai` vision suite (`tests/common/fixtures.rs`).
const VISION_IMAGE_BYTES: &[u8] = include_bytes!("../../../../../vercel-ai/ai/share.jpg");

fn params_for(prompt: Vec<LlmMessage>, source: &str) -> QueryParams {
    QueryParams {
        prompt,
        max_tokens: Some(1024),
        thinking_level: None,
        fast_mode: false,
        tools: None,
        tool_choice: None,
        context_management: None,
        query_source: Some(source.to_string()),
        agent_id: None,
        time_since_last_assistant_ms: None,
        agentic: false,
        cache: None,
        stop_sequences: None,
        response_format: None,
        cancel: None,
        wire_tap: None,
    }
}

/// Send a user turn that mixes `input_text` + `input_image` (inline
/// JPEG) and assert the model can read the year off the image.
/// Matches the documented Responses API curl, substituting an inline
/// data URL for the remote URL.
pub async fn run(target: &LiveTarget) -> Result<()> {
    let prompt = vec![
        LlmMessage::system(SYSTEM),
        LlmMessage::user(vec![
            UserContentPart::text("What year is shown in this image? Answer with just the number."),
            UserContentPart::image(VISION_IMAGE_BYTES.to_vec(), "image/jpeg"),
        ]),
    ];

    let result = query_client(
        &target.client,
        params_for(prompt, "coco-tests-live::sdk::vision::run"),
    )
    .await?;
    usage_report::record(target.provider, &target.model, "vision.run", &result.usage);

    let text = extract_text(&result);
    assert!(
        !text.trim().is_empty(),
        "{}/{}: vision call returned empty text",
        target.provider,
        target.model
    );
    // Catches the "model didn't actually receive the image bytes"
    // failure mode — the fixture shows "2025"; any vision-capable model
    // that read it should echo the digits.
    assert!(
        text.contains("2025"),
        "{}/{}: vision answer didn't mention the year on the fixture; got: {text}",
        target.provider,
        target.model
    );
    Ok(())
}
