use super::*;
use vercel_ai_provider::ReasoningPart;
use vercel_ai_provider::TextPart;

#[test]
fn test_extract_reasoning_content_empty() {
    let content: Vec<AssistantContentPart> = vec![];
    assert!(extract_reasoning_content(&content).is_empty());
}

#[test]
fn test_extract_reasoning_content_single() {
    let content = vec![AssistantContentPart::Reasoning(ReasoningPart {
        text: "Thinking...".to_string(),
        provider_metadata: None,
    })];
    assert_eq!(extract_reasoning_content(&content), vec!["Thinking..."]);
}

#[test]
fn test_extract_reasoning_content_multiple() {
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "First thought".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "Second thought".to_string(),
            provider_metadata: None,
        }),
    ];
    assert_eq!(
        extract_reasoning_content(&content),
        vec!["First thought", "Second thought"]
    );
}

#[test]
fn test_extract_reasoning_content_mixed() {
    let content = vec![
        AssistantContentPart::Text(TextPart {
            text: "Some text".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "Hidden reasoning".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Text(TextPart {
            text: "More text".to_string(),
            provider_metadata: None,
        }),
    ];
    assert_eq!(
        extract_reasoning_content(&content),
        vec!["Hidden reasoning"]
    );
}

#[test]
fn test_extract_reasoning_text() {
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "First".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "Second".to_string(),
            provider_metadata: None,
        }),
    ];
    assert_eq!(extract_reasoning_text(&content), "First\nSecond");
}

#[test]
fn test_has_reasoning_content() {
    let with_reasoning = vec![AssistantContentPart::Reasoning(ReasoningPart {
        text: "thinking".to_string(),
        provider_metadata: None,
    })];
    let without_reasoning = vec![AssistantContentPart::Text(TextPart {
        text: "text".to_string(),
        provider_metadata: None,
    })];

    assert!(has_reasoning_content(&with_reasoning));
    assert!(!has_reasoning_content(&without_reasoning));
}

#[test]
fn test_extract_reasoning_with_stats() {
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "abc".to_string(),
            provider_metadata: None,
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "defgh".to_string(),
            provider_metadata: None,
        }),
    ];

    let (reasoning, char_count) = extract_reasoning_with_stats(&content);
    assert_eq!(reasoning, vec!["abc", "defgh"]);
    assert_eq!(char_count, 8);
}
