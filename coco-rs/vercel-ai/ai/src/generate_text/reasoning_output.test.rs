use super::*;
use vercel_ai_provider::ReasoningPart;

#[test]
fn test_reasoning_output_new() {
    let output = ReasoningOutput::new("Thinking...");
    assert_eq!(output.text, "Thinking...");
    assert!(output.signature.is_none());
    assert!(output.provider_metadata.is_none());
}

#[test]
fn test_reasoning_output_with_signature() {
    let output = ReasoningOutput::new("Thinking...").with_signature("sig123");
    assert_eq!(output.signature, Some("sig123".to_string()));
}

#[test]
fn test_extract_reasoning_outputs() {
    let content = vec![
        AssistantContentPart::text("Hello"),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "First thought".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "Second thought".to_string(),
            provider_metadata: None,
        }),
    ];

    let items = convert_to_reasoning_outputs(&content);
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].as_text().map(|r| r.text.as_str()),
        Some("First thought")
    );
    assert_eq!(
        items[1].as_text().map(|r| r.text.as_str()),
        Some("Second thought")
    );
}

#[test]
fn test_reasoning_text() {
    let outputs = vec![
        ReasoningOutput::new("First"),
        ReasoningOutput::new("Second"),
    ];
    assert_eq!(reasoning_text(&outputs), "First\nSecond");
}

#[test]
fn test_reasoning_text_empty() {
    let outputs: Vec<ReasoningOutput> = vec![];
    assert_eq!(reasoning_text(&outputs), "");
}
