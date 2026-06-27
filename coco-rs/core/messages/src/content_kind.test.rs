use super::*;
use crate::FileContent;
use crate::ReasoningContent;
use crate::TextContent;
use crate::ToolCallContent;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn text_density_is_quarter_chars() {
    assert_eq!(estimate_part(ContentKind::Text, 400), 100);
    assert_eq!(estimate_part(ContentKind::Text, 0), 0);
    // Integer truncation: 7/4 = 1
    assert_eq!(estimate_part(ContentKind::Text, 7), 1);
}

#[test]
fn json_density_is_half_chars() {
    assert_eq!(estimate_part(ContentKind::Json, 400), 200);
    assert_eq!(estimate_part(ContentKind::Json, 100), 50);
    assert_eq!(estimate_part(ContentKind::Json, 0), 0);
}

#[test]
fn image_density_is_fixed_constant() {
    // Chars arg is ignored.
    assert_eq!(estimate_part(ContentKind::Image, 0), IMAGE_MAX_TOKEN_SIZE);
    assert_eq!(
        estimate_part(ContentKind::Image, 1_000_000),
        IMAGE_MAX_TOKEN_SIZE,
    );
    assert_eq!(IMAGE_MAX_TOKEN_SIZE, 2_000);
}

#[test]
fn classify_user_text_picks_text_kind() {
    let part = UserContent::Text(TextContent::new("hello world"));
    let (kind, chars) = classify_user(&part);
    assert_eq!(kind, ContentKind::Text);
    assert_eq!(chars, "hello world".len() as i64);
}

#[test]
fn classify_user_file_dispatches_on_extension() {
    let json_file = UserContent::File(
        FileContent::from_url("file:///x", "application/json").with_filename("config.json"),
    );
    assert_eq!(classify_user(&json_file).0, ContentKind::Json);

    let png_file = UserContent::File(
        FileContent::from_url("file:///x", "image/png").with_filename("screenshot.png"),
    );
    assert_eq!(classify_user(&png_file).0, ContentKind::Image);

    let no_filename = UserContent::File(FileContent::from_url(
        "file:///x",
        "application/octet-stream",
    ));
    assert_eq!(classify_user(&no_filename).0, ContentKind::Image);
}

#[test]
fn classify_user_file_extension_is_case_insensitive() {
    let part = UserContent::File(
        FileContent::from_url("file:///x", "application/json").with_filename("CONFIG.JSON"),
    );
    assert_eq!(classify_user(&part).0, ContentKind::Json);
}

#[test]
fn classify_assistant_tool_call_splits_into_name_and_json_input() {
    let part = AssistantContent::ToolCall(ToolCallContent::new(
        "id",
        "Read",
        json!({"file_path": "/tmp/x.txt"}),
    ));
    let parts = classify_assistant(&part);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].0, ContentKind::Text); // tool_name
    assert_eq!(parts[0].1, "Read".len() as i64);
    assert_eq!(parts[1].0, ContentKind::Json);
    assert!(parts[1].1 > 0);
}

#[test]
fn classify_assistant_reasoning_is_text() {
    let part = AssistantContent::Reasoning(ReasoningContent {
        text: "step by step reasoning".into(),
        provider_metadata: None,
    });
    let parts = classify_assistant(&part);
    assert_eq!(
        parts,
        vec![(ContentKind::Text, "step by step reasoning".len() as i64)],
    );
}

#[test]
fn classify_tool_result_text_is_text() {
    let output = ToolResultOutput::Text {
        value: "hello".into(),
        provider_options: None,
    };
    assert_eq!(classify_tool_result(&output), vec![(ContentKind::Text, 5)],);
}

#[test]
fn classify_tool_result_json_is_json() {
    let output = ToolResultOutput::Json {
        value: json!({"a": 1, "b": 2}),
        provider_options: None,
    };
    let parts = classify_tool_result(&output);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].0, ContentKind::Json);
    assert!(parts[0].1 > 0);
}

#[test]
fn classify_tool_result_content_with_filedata_part_returns_image_cost() {
    let output = ToolResultOutput::Content {
        value: vec![
            ToolResultContentPart::text("see screenshot"),
            ToolResultContentPart::file_data("BASE64", "image/png"),
        ],
        provider_options: None,
    };
    let parts = classify_tool_result(&output);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], (ContentKind::Text, "see screenshot".len() as i64));
    assert_eq!(parts[1].0, ContentKind::Image);
}
