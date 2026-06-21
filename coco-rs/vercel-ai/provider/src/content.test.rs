use super::*;
use serde_json::json;

#[test]
fn test_text_part_serialization() {
    let part = TextPart::new("Hello, world!");
    let json = serde_json::to_string(&part).unwrap();
    let parsed: TextPart = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.text, "Hello, world!");
}

#[test]
fn test_user_content_part_text() {
    let part = UserContentPart::text("Hello");
    let json = serde_json::to_string(&part).unwrap();

    // The enum uses #[serde(tag = "type")] so "type" is at the enum level
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("Hello"));
}

#[test]
fn test_user_content_part_file() {
    let part = UserContentPart::image(vec![1, 2, 3, 4], "image/png");
    let json = serde_json::to_string(&part).unwrap();

    assert!(json.contains("\"type\":\"file\""));
    assert!(json.contains("image/png"));
}

#[test]
fn test_assistant_content_part_text() {
    let part = AssistantContentPart::text("Response");
    let json = serde_json::to_string(&part).unwrap();

    // The enum uses #[serde(tag = "type")] so "type" is at the enum level
    assert!(json.contains("\"type\":\"text\""));
}

#[test]
fn test_assistant_content_part_reasoning() {
    let part = AssistantContentPart::reasoning("Thinking...");
    let json = serde_json::to_string(&part).unwrap();

    assert!(json.contains("\"type\":\"reasoning\""));
    assert!(json.contains("Thinking..."));
}

#[test]
fn test_assistant_content_part_tool_call() {
    let part = AssistantContentPart::tool_call("call_123", "search", json!({"query": "test"}));
    let json = serde_json::to_string(&part).unwrap();

    // The enum uses #[serde(tag = "type", rename_all = "kebab-case")]
    assert!(json.contains("\"type\":\"tool-call\""));
    assert!(json.contains("call_123"));
    assert!(json.contains("search"));
}

#[test]
fn test_reasoning_part() {
    let part = ReasoningPart::new("Let me think about this...");
    assert_eq!(part.text, "Let me think about this...");
}

#[test]
fn test_tool_call_part() {
    let part = ToolCallPart::new("call_456", "read_file", json!({"path": "/tmp/file.txt"}));
    assert_eq!(part.tool_call_id, "call_456");
    assert_eq!(part.tool_name, "read_file");
    assert!(part.provider_executed.is_none());

    // Test with provider_executed
    let provider_part = ToolCallPart::new("call_789", "mcp_tool", json!({"arg": "value"}))
        .with_provider_executed(true);
    assert_eq!(provider_part.provider_executed, Some(true));
}

#[test]
fn test_tool_result_part() {
    let part = ToolResultPart::new(
        "call_789",
        "read_file",
        ToolResultContent::text("file contents"),
    );
    assert!(!part.is_error);

    let error_part = part.with_error();
    assert!(error_part.is_error);
}

#[test]
fn test_tool_result_content() {
    // Text content
    let text = ToolResultContent::text("some text");
    let json = serde_json::to_string(&text).unwrap();
    let parsed: ToolResultContent = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, ToolResultContent::Text { .. }));

    // JSON content
    let json_content = ToolResultContent::json(json!({"status": "ok"}));
    let json_str = serde_json::to_string(&json_content).unwrap();
    let parsed: ToolResultContent = serde_json::from_str(&json_str).unwrap();
    assert!(matches!(parsed, ToolResultContent::Json { .. }));

    // Execution denied
    let denied = ToolResultContent::execution_denied(Some("User cancelled".to_string()));
    let json_str = serde_json::to_string(&denied).unwrap();
    let parsed: ToolResultContent = serde_json::from_str(&json_str).unwrap();
    assert!(matches!(parsed, ToolResultContent::ExecutionDenied { .. }));

    // Error text
    let error = ToolResultContent::error_text("Something went wrong");
    let json_str = serde_json::to_string(&error).unwrap();
    let parsed: ToolResultContent = serde_json::from_str(&json_str).unwrap();
    assert!(matches!(parsed, ToolResultContent::ErrorText { .. }));

    // Content parts
    let parts = ToolResultContent::content_parts(vec![
        ToolResultContentPart::text("Hello"),
        ToolResultContentPart::file_url("https://example.com/image.png", "image/png"),
    ]);
    let json_str = serde_json::to_string(&parts).unwrap();
    let parsed: ToolResultContent = serde_json::from_str(&json_str).unwrap();
    assert!(matches!(parsed, ToolResultContent::Content { .. }));
}

#[test]
fn test_tool_approval_response() {
    let approved = ToolApprovalResponsePart::new("approval_123", true);
    assert!(approved.approved);
    assert_eq!(approved.approval_id, "approval_123");
    assert!(approved.reason.is_none());

    let denied = ToolApprovalResponsePart::new("approval_456", false)
        .with_reason("User cancelled the operation");
    assert!(!denied.approved);
    assert_eq!(
        denied.reason,
        Some("User cancelled the operation".to_string())
    );
}

#[test]
fn test_file_part() {
    let part = FilePart::image(vec![0x89, 0x50, 0x4E, 0x47], "image/png");
    assert_eq!(part.media_type, "image/png");
}

#[test]
fn test_source_part_url() {
    let part = SourcePart::url_source("src_123", "https://example.com/page");
    assert_eq!(part.source_type, SourceType::Url);
    assert_eq!(part.id, "src_123");
    assert_eq!(part.url, Some("https://example.com/page".to_string()));
}

#[test]
fn test_source_part_document() {
    let part = SourcePart::document("doc_456", "Report.pdf", "application/pdf");
    assert_eq!(part.source_type, SourceType::Document);
    assert_eq!(part.id, "doc_456");
    assert_eq!(part.title, Some("Report.pdf".to_string()));
    assert_eq!(part.media_type, Some("application/pdf".to_string()));
}

#[test]
fn test_assistant_content_part_source() {
    let part = AssistantContentPart::source("src_789", SourceType::Url);
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("\"type\":\"source\""));
    assert!(json.contains("\"src_789\""));
}

#[test]
fn test_tool_approval_request_part() {
    let part = ToolApprovalRequestPart::new("approval_123", "call_456")
        .with_tool_name("delete_file")
        .with_context("This will permanently delete the file");

    assert_eq!(part.approval_id, "approval_123");
    assert_eq!(part.tool_call_id, "call_456");
    assert_eq!(part.tool_name, Some("delete_file".to_string()));
    assert_eq!(
        part.context,
        Some("This will permanently delete the file".to_string())
    );
}

#[test]
fn test_assistant_content_part_tool_approval_request() {
    let part = AssistantContentPart::tool_approval_request("approval_789", "call_999");
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("\"type\":\"tool-approval-request\""));
    assert!(json.contains("\"approval_789\""));
}

#[test]
fn test_source_type_serialization() {
    let url_type = SourceType::Url;
    let json = serde_json::to_string(&url_type).unwrap();
    assert_eq!(json, "\"url\"");

    let doc_type = SourceType::Document;
    let json = serde_json::to_string(&doc_type).unwrap();
    assert_eq!(json, "\"document\"");
}

#[test]
fn test_custom_part_creation() {
    let part = CustomPart::new("openai-compaction");
    assert_eq!(part.kind, "openai-compaction");
    assert!(part.provider_options.is_none());
    assert!(part.provider_metadata.is_none());
}

#[test]
fn test_assistant_content_part_custom_serialization() {
    let part = AssistantContentPart::custom("test-kind");
    let json = serde_json::to_string(&part).unwrap();
    assert!(json.contains("\"type\":\"custom\""));
    assert!(json.contains("\"kind\":\"test-kind\""));

    let deserialized: AssistantContentPart = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        deserialized,
        AssistantContentPart::Custom(CustomPart { kind, .. }) if kind == "test-kind"
    ));
}
