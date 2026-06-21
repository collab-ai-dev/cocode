use serde_json::json;

use super::*;

#[test]
fn test_text_part_new() {
    let part = PromptTextPart::new("Hello");
    assert_eq!(part.part_type, "text");
    assert_eq!(part.text, "Hello");
    assert!(part.provider_options.is_none());
}

#[test]
fn test_image_part_from_base64() {
    let part = PromptImagePart::from_base64("SGVsbG8=", "image/png");
    assert_eq!(part.part_type, "image");
    assert!(matches!(part.image, PromptImageData::Base64(ref s) if s == "SGVsbG8="));
    assert_eq!(part.media_type, Some("image/png".to_string()));
}

#[test]
fn test_image_part_from_url() {
    let part = PromptImagePart::from_url("https://example.com/image.png");
    assert_eq!(part.part_type, "image");
    assert!(
        matches!(part.image, PromptImageData::Url(ref u) if u == "https://example.com/image.png")
    );
    assert!(part.media_type.is_none());
}

#[test]
fn test_file_part_from_base64() {
    let part = PromptFilePart::from_base64("data", "application/pdf", Some("doc.pdf".to_string()));
    assert_eq!(part.part_type, "file");
    assert_eq!(part.media_type, "application/pdf");
    assert_eq!(part.filename, Some("doc.pdf".to_string()));
}

#[test]
fn test_file_part_from_url() {
    let part = PromptFilePart::from_url("https://example.com/doc.pdf", "application/pdf");
    assert_eq!(part.part_type, "file");
    assert!(matches!(part.data, PromptFileData::Url(ref u) if u == "https://example.com/doc.pdf"));
    assert!(part.filename.is_none());
}

#[test]
fn test_reasoning_part_new() {
    let part = PromptReasoningPart::new("Let me think...");
    assert_eq!(part.part_type, "reasoning");
    assert_eq!(part.text, "Let me think...");
}

#[test]
fn test_tool_call_part_new() {
    let part = PromptToolCallPart::new("tc1", "my_tool", json!({"key": "value"}));
    assert_eq!(part.part_type, "tool-call");
    assert_eq!(part.tool_call_id, "tc1");
    assert_eq!(part.tool_name, "my_tool");
    assert_eq!(part.input, json!({"key": "value"}));
}

#[test]
fn test_tool_result_part_text_output() {
    let part = PromptToolResultPart::new(
        "tc1",
        "my_tool",
        PromptToolResultOutput::Text {
            value: "result text".to_string(),
        },
    );
    assert_eq!(part.part_type, "tool-result");
    assert_eq!(part.tool_call_id, "tc1");
    assert_eq!(part.tool_name, "my_tool");
    assert!(matches!(part.output, PromptToolResultOutput::Text { .. }));
}

#[test]
fn test_tool_result_part_json_output() {
    let part = PromptToolResultPart::new(
        "tc1",
        "my_tool",
        PromptToolResultOutput::Json {
            value: json!({"result": true}),
        },
    );
    assert!(matches!(part.output, PromptToolResultOutput::Json { .. }));
}

#[test]
fn test_tool_result_output_serde_roundtrip() {
    let outputs = vec![
        PromptToolResultOutput::Text {
            value: "hello".to_string(),
        },
        PromptToolResultOutput::Json {
            value: json!({"key": "val"}),
        },
        PromptToolResultOutput::ErrorText {
            value: "error msg".to_string(),
        },
        PromptToolResultOutput::ExecutionDenied {
            reason: Some("not allowed".to_string()),
        },
    ];

    for output in outputs {
        let json = serde_json::to_value(&output).unwrap();
        let deserialized: PromptToolResultOutput = serde_json::from_value(json).unwrap();
        // Just verify it round-trips without error
        let _ = serde_json::to_string(&deserialized).unwrap();
    }
}
