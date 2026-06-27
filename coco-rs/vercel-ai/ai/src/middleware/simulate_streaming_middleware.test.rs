use super::*;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::Usage;

fn create_test_result() -> LanguageModelV4GenerateResult {
    LanguageModelV4GenerateResult {
        content: vec![AssistantContentPart::Text(TextPart {
            text: "Hello world".to_string(),
            provider_metadata: None,
        })],
        finish_reason: FinishReason::end_turn(),
        usage: Usage::new(10, 5),
        request: Default::default(),
        response: Default::default(),
        warnings: vec![],
        provider_metadata: None,
    }
}

#[test]
fn test_simulate_stream_creates_parts() {
    let result = create_test_result();
    let parts = simulate_stream(result);

    // Should have: StreamStart, ResponseMetadata, TextStart, TextDelta, TextEnd, Finish
    assert!(parts.len() >= 6);

    // Check first part is StreamStart
    matches!(parts[0], LanguageModelV4StreamPart::StreamStart { .. });

    // Check last part is Finish
    matches!(parts.last(), Some(LanguageModelV4StreamPart::Finish { .. }));
}
